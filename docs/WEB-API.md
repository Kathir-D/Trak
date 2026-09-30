# trak — Spotify Web API reality check (TODO 1.7 / 7.1)

**Researched 2026-09-29 against `developer.spotify.com`.** Every claim below cites
the page it came from; where the docs are ambiguous or self-contradictory, that is
called out rather than smoothed over. Nothing here is from memory.

This doc is the authority for what Version A can do. `docs/SPEC.md` §6 and Phase 7
of `TODO.md` were updated to match it in the same commit.

---

## 0. Headline: the plan's assumptions were stale in four ways that change the build

1. **`http://localhost:<port>` is no longer a legal redirect URI.** Only loopback IP
   literals are allowed over HTTP, and `localhost` is named explicitly as
   disallowed. `TODO.md` 7.2 said "loopback listener on a free port (or the fixed
   port the redirect URI needs)" — the *first* option is the only one that works
   without registering a fixed port.
2. **Developer mode requires the app owner to have Spotify Premium, and caps the
   app at 5 allowlisted users** (not 25). Live for new Client IDs since
   2026-02-11 and for all existing dev-mode integrations since 2026-03-09.
3. **A large set of endpoints was removed in dev mode**, including every
   multi-item "get several" endpoint, `GET /markets`, `GET /users/{id}`,
   `GET /users/{id}/playlists`, and **`GET /artists/{id}/top-tracks`**. The
   library/follow/contains endpoints were consolidated into `PUT`/`DELETE
   /me/library` and `GET /me/library/contains`.
4. **`Track.popularity` and `Album.popularity` are removed in dev mode.** This one
   cuts the other way: trak already gets a real popularity from AppleScript
   (`docs/APPLESCRIPT.md` §2), so the Info tab keeps working without the API.

Not stale: PKCE is still the recommended flow, and access tokens still last one
hour.

---

## 1. Redirect URIs — `localhost` is banned

Source: <https://developer.spotify.com/documentation/web-api/concepts/redirect_uri>
(accessed 2026-09-29). Requirements, quoted:

> - Use HTTPS for your redirect URI, unless you are using a loopback address, when
>   HTTP is permitted.
> - If you are using a loopback address, use the explicit IPv4 or IPv6, like
>   `http://127.0.0.1:PORT` or `http://[::1]:PORT` as your redirect URI.
> - **`localhost` is not allowed as redirect URI.**

Enforcement: "Beginning on the 9th of April 2025 we will enforce the subsequent
validations to all newly created apps… all clients to migrate by **November
2025**." Both dates are past, so this is live.

**Dynamic ports are explicitly supported**, and this is the answer for trak:

> "If you don't know the port number in advance, register your redirect URI with a
> loopback IP literal, **but without any port number**. You can add the
> dynamically assigned port number to the redirect URI in the authorization
> request."

So the guided setup in `trak config` (TODO 7.3) must tell the user to register
exactly:

```
http://127.0.0.1
```

with no port and no path, and trak then binds a free ephemeral port per login and
sends the matching `redirect_uri` in the authorization request. The alternative
(a fixed port) means `trak` fails to log in whenever that port is taken, which on a
laptop is a real occurrence. Prefer the dynamic port.

The exact-match rule still holds for everything that is not a loopback IP literal
("The definition of the redirect URI must exactly match… The only exception is for
loopback IP literals, which can dynamically be assigned ports").

**Unresolved:** the page renders a stray `Error:` string at the top of its body
(a docs-site build artefact). It is not clear whether the dashboard accepts a bare
`http://127.0.0.1` with no path, or insists on something like
`http://127.0.0.1/callback`. The prose says to register it "without any port
number" and does not mention a path, so read the no-path form as intended, but
this is the one thing to confirm during the first real login (**[owner]**, TODO 7.3).

## 2. Developer mode: Premium required, 5 users

Sources: <https://developer.spotify.com/documentation/web-api/concepts/quota-modes>,
<https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide>,
<https://developer.spotify.com/blog/2026-02-06-update-on-developer-access-and-platform-security>,
<https://developer.spotify.com/documentation/web-api/references/changes/july-2026>
(all accessed 2026-09-29).

**Premium is required of the app owner:**

> "The app owner must have a Spotify Premium account for apps in development mode to
> function." — quota-modes

> "If the owner's Premium subscription lapses, the app will stop working. It will
> resume functioning once the owner resubscribes." — migration guide

For trak this is fine — the owner is the app owner and is the only user.

**User cap is 5, down from 25:**

> "Up to 5 authenticated Spotify users can use an app that is in development mode…
> Each Spotify user who installs your app will need to be added to your app's
> allowlist before they can use it." — quota-modes

The failure mode is a 403: "Users may be able to log into a development mode app
without having been allowlisted… However, API requests with an access token
associated to that user and app will receive a **403** status code error."

This is what TODO 7.5's "403 → friendly typed error" must actually handle in
practice, so the message should say *"this Spotify account is not on trak's
allowlist"* rather than a generic "permission denied".

**Client IDs per developer: 25** (raised from 1 in July 2026).

**Quota is per developer account, not per Client ID** (since July 2026), and
exhaustion has its own error body, distinct from a rate limit:

```json
{ "error": { "status": 429, "message": "Too many requests", "reason": "QUOTA_EXCEEDED" } }
```

**Extended quota mode** (unlimited users, no allowlist) requires all six of:
established business entity, launched service, ≥250k MAUs, key-market
availability, commercial viability, ToS compliance. Since 2025-05-15 it is
organisations only, not individuals. **trak is not eligible and should never
promise this path.**

**One postponement to be aware of:** the 6 Feb 2026 blog carries a dated update
saying the *endpoint access* changes for **pre-existing** integrations were
postponed, while the Premium requirement, the 5-user cap and the Client ID limit
proceeded on 2026-03-09. Since trak's Client ID will be created *after* that, the
full restricted endpoint set applies to trak.

## 3. Endpoint availability

Authoritative source: the "Endpoints still available" list in
<https://developer.spotify.com/documentation/web-api/references/changes/february-2026>
(accessed 2026-09-29). Scopes from each endpoint's reference page.

| trak feature (SPEC §6) | Endpoint | In dev mode? | Scope | Premium-only? |
| --- | --- | --- | --- | --- |
| Grouped search | `GET /search` | ✅ | none | No |
| My playlists (list) | `GET /me/playlists` | ✅ | `playlist-read-private` | No |
| Playlist detail | `GET /playlists/{id}` | ✅ | none | No |
| Queue view | `GET /me/player/queue` | ✅ | `user-read-currently-playing`, `user-read-playback-state` | Not stated |
| **Add to queue (`A`)** | `POST /me/player/queue` | ✅ | `user-modify-playback-state` | **Yes** — "This API only works for users who have Spotify Premium." |
| Liked songs (list) | `GET /me/tracks` | ✅ | `user-library-read` | No |
| **Like/unlike (`f`)** | `PUT`/`DELETE /me/tracks` | ❌ removed → **`PUT`/`DELETE `/me/library`** | `user-library-modify` | No |
| **Is-liked check** | `GET /me/tracks/contains` | ❌ removed → **`GET /me/library/contains`** | `user-library-read` | No |
| Saved albums | `GET /me/albums` | ✅ | `user-library-read` | No |
| Followed artists (read) | `GET /me/following` | ✅ | `user-follow-read` | No |
| Follow artists (write) | `PUT`/`DELETE /me/following` | ❌ removed → `PUT`/`DELETE /me/library` | `user-follow-modify` | No |
| Recently played | `GET /me/player/recently-played` | ✅ | `user-read-recently-played` | No (note: "Currently doesn't support podcast episodes") |
| **Artist top tracks** | `GET /artists/{id}/top-tracks` | ❌ **removed, no replacement** | — | — |
| Artist albums | `GET /artists/{id}/albums` | ✅ | none | No |
| Album detail | `GET /albums/{id}` | ✅ | none | No |
| Album tracks | `GET /albums/{id}/tracks` | ✅ | none | No |
| Create playlist | `POST /me/playlists` | ✅ | `playlist-modify-public` / `-private` | No |
| Playlist add/remove/replace items | `POST`/`DELETE`/`PUT /playlists/{id}/items` | ⚠️ **see the ambiguity below** | `playlist-modify-public` / `-private` | No |
| Current user profile | `GET /me` | ✅ | none | No |
| Top items | `GET /me/top/{type}` | ✅ | `user-top-read` | No |

### Removed in dev mode (Feb 2026) that trak must not call

- **All multi-item fetches**: `GET /tracks`, `GET /albums`, `GET /artists`,
  `GET /episodes`, `GET /shows`, `GET /audiobooks`, `GET /chapters`. Replaced by
  one request per item. **This is the one most likely to break `rspotify`
  silently**, because the crate's convenience methods take a list of ids and are
  built on the batch endpoints. `web/api.rs` must avoid them and loop.
- `GET /markets`, `GET /browse/new-releases`, `GET /browse/categories`,
  `GET /browse/categories/{id}`, `GET /users/{id}`, `GET /users/{id}/playlists`,
  `POST /users/{user_id}/playlists`.
- `GET /artists/{id}/top-tracks` — **no replacement is offered.** SPEC §6 lists
  "artist page (top tracks, albums)" as a Version A feature; it can no longer
  offer top tracks. SPEC §6 and TODO 7.10 are updated accordingly.
- Playlist track management moved from `/playlists/{id}/tracks` to
  `/playlists/{id}/items` (query param `tracks` → `items`).

### Removed *response fields* (dev mode)

- **Track**: `available_markets`, `linked_from`, **`popularity`**
- **Album**: `album_group`, `available_markets`, `label`, **`popularity`**
- **Artist**: `followers`, **`popularity`**
- **User (`GET /me`)**: `country`, `email`, `explicit_content`, `followers`,
  `product`
- Show / Audiobook / Chapter: `available_markets`, `publisher`
- `external_ids` on Track and Album was removed then **reverted** in March 2026
  and is available.

Note `User.product` is gone, so the API can no longer be used to detect Premium.
trak must not depend on it — the Premium-only failure surfaces as the 403 on
`POST /me/player/queue` instead, which is the honest signal anyway.

### Renamed response fields

- Playlist: `tracks` → `items`, `tracks.tracks` → `items.items`,
  `tracks.tracks.track` → `items.items.item`.
- `GET /me` gained `account_id` in May 2026: "a public, immutable, pseudoanonymous
  identifier… stable and will not change over the lifetime of the account."
  (trak does not need it; noted so nobody mistakes its absence for a bug.)

### ⚠️ The one genuine ambiguity: playlist item endpoints

`POST`/`GET`/`PUT`/`DELETE /playlists/{id}/items` are the documented replacement
and are **not** marked deprecated. But they are **absent from the "Endpoints still
available" list** in the February 2026 changelog — that list's Playlist section
contains only `PUT /playlists/{id}`, `POST /me/playlists`, `GET /me/playlists`,
`GET /playlists/{id}`, `GET /playlists/{id}/images`, `PUT /playlists/{id}/images`.

So the official docs simultaneously tell you to migrate to `/items` and omit it
from the dev-mode availability list. Secondary evidence is mixed: a user reported
in Aug 2026 that switching from `/tracks` to `/items` "resolved the 403 Forbidden
error"; earlier Feb–May 2026 reports had `/items` itself 403ing.
**Treat playlist read/write as unverified until a real dev-mode login tests it**
(TODO 7.7, 7.11). Do not design the UI assuming it works, and do not assume it
fails — make it a capability the user turns on.

A separate restriction *is* well documented, and is a bigger deal for a music
browser: **playlist contents are only returned for playlists the user owns or
collaborates on.** "For other playlists, only metadata is returned and the `items`
field will be absent." trak's Playlists tab is the user's own, so this is fine, but
no feature may promise "show me any playlist's tracks".

## 4. Rate limits: there are no published numbers

Source: <https://developer.spotify.com/documentation/web-api/concepts/rate-limits>
(accessed 2026-09-29).

Spotify **does not publish numeric rate limits.** The page says the limit "is
calculated based on the number of calls that your app makes to Spotify in a rolling
30 second window" and "varies depending on whether your app is in development mode
or extended quota mode", illustrated with a graph rather than numbers.

Only `Retry-After` is documented: "The header of the 429 response will normally
include a `Retry-After` header with a value in seconds."

**`X-RateLimit-*` headers are not documented anywhere on developer.spotify.com**,
and there is no remaining-quota endpoint. The dashboard graph is the only
visibility, and it is manual. So TODO 7.5 must implement 429 handling purely from
`Retry-After`, with a bounded backoff and a cap — there is no quota counter to
read, and trak should not pretend otherwise or show a fake "requests left" number.

429 is now disambiguable: check the body for `"reason": "QUOTA_EXCEEDED"` to tell
dev-mode quota exhaustion from a rate limit, since they are enforced separately.

## 5. PKCE: unchanged, still the right choice

Source: <https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow>
(accessed 2026-09-29).

> "The authorization code flow with PKCE is the recommended authorization flow if
> you're implementing authorization in a mobile app, single page web apps, or **any
> other type of application where the client secret can't be safely stored**."

The Feb 2025 security post confirms public clients are expected to use PKCE. No
2026 change to the mechanism itself: code verifier → S256 challenge →
`code_challenge_method=S256`. `client_id` is required on the token request for
PKCE clients; confidential clients use HTTP Basic instead.

The implicit grant flow is now explicitly **Deprecated** in the docs navigation.

## 6. Tokens: the refresh token expires in 6 months

Sources: <https://developer.spotify.com/documentation/web-api/tutorials/refreshing-tokens>,
<https://developer.spotify.com/documentation/web-api/concepts/access-token>
(accessed 2026-09-29).

- **Access token: 1 hour (3600 s)**, unchanged.
- **Refresh token: 6 months**, and this is the constraint that matters. Verbatim:
  > "Refresh tokens issued to apps registered in the Developer Dashboard have a
  > lifetime of **6 months**."
  > - "The 6-month lifetime starts when the user authorizes your app."
  > - "Refreshing an access token does not extend the refresh token's lifetime."
  > - "After 6 months, the refresh token can no longer be used."
  > - "Your app must send the user through the authorization flow again."

  With a banner: "Build reauthorization into your app before refresh tokens
  expire. Do not assume that a refresh token remains valid indefinitely."

- On an invalid or expired refresh token: "Your app should discard the refresh
  token and start the appropriate reauthorization."

**Consequences for trak**, which are not in the current plan:

- A token in Keychain or a `0600` file is **not durable for six months**. trak
  needs an explicit "reconnect Spotify" state in the TUI and in the settings
  screen, not a silent failure.
- Because the redirect URI carries a dynamic port (§1), re-running the login flow
  is cheap — no port conflict to negotiate. That makes a hard reconnect cheap
  enough that trak can just do it.
- TODO 7.2 should record the refresh-token issue time locally (it is not returned
  as a field, only implied by the authorization moment) and warn before expiry
  rather than only failing at it.

---

## 7. Token storage (TODO 1.8 / 7.4)

The ad-hoc-signing / Keychain decision from TODO 1.8 is written up in
`docs/KEYCHAIN.md`, and the `Store` trait that abstracts the two backends is
planned in TODO 7.4. The short version: **the Keychain works from an ad-hoc-signed
binary, and the re-prompt risk in R3 is real but does not affect trak's choice** —
see that doc for the measurements.

## 8. What trak should do about all this

Concretely, for the next agent building Phase 7:

1. **Redirect URI is `http://127.0.0.1`, no port, no path**, registered once in
   the dashboard. Bind an ephemeral port per login. Never send `localhost`.
2. **Do not use `rspotify`'s batch helpers** (`tracks(ids)`, `artists(ids)`,
   `albums(ids)`) — those call removed endpoints. Loop one id per request, and
   cache aggressively: the dev-mode quota is per developer account and shared by
   every Client ID.
3. **Library writes go through `/me/library`**, not `/me/tracks`. `f` (like) is
   `PUT /me/library` with `ids: [spotify:track:…]`, and the is-liked check is
   `GET /me/library/contains?ids=…&types=track`.
4. **Drop artist top tracks.** TODO 7.10's artist page is albums-only. `GET
   /artists/{id}/top-tracks` returns 404/403 in dev mode with no replacement.
5. **The 403 on `POST /me/player/queue` is expected for Free users.** SPEC §6
   already promised a one-line message; keep that, and say it is a Premium
   limitation rather than an error.
6. **Search `limit` max is 10, default 5** (was 50/20). The grouped search tab
   must paginate or just ask for 10 per group.
7. **429 handling uses only `Retry-After`.** No quota introspection exists. Cap
   the backoff, and surface a one-line "rate limited, retrying in Ns".
8. **Refresh tokens last 6 months.** Build the reconnect path now; it is not an
   edge case.
9. **Playlist item read/write is unverified.** Ship it behind a capability check
   and confirm during the first real login.

## 9. Sources

All accessed 2026-09-29.

| Claim area | URL |
| --- | --- |
| Redirect URIs | <https://developer.spotify.com/documentation/web-api/concepts/redirect_uri> |
| Redirect URI migration | <https://developer.spotify.com/documentation/web-api/tutorials/migration-insecure-redirect-uri> |
| 2025 security post | <https://developer.spotify.com/blog/2025-02-12-increasing-the-security-requirements-for-integrating-with-spotify> |
| Quota modes | <https://developer.spotify.com/documentation/web-api/concepts/quota-modes> |
| Feb 2026 changelog (endpoint removals) | <https://developer.spotify.com/documentation/web-api/references/changes/february-2026> |
| Feb 2026 migration guide | <https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide> |
| Mar 2026 changelog (reverts) | <https://developer.spotify.com/documentation/web-api/references/changes/march-2026> |
| May 2026 changelog (`account_id`) | <https://developer.spotify.com/documentation/web-api/references/changes/may-2026> |
| Jul 2026 changelog (Client ID cap) | <https://developer.spotify.com/documentation/web-api/references/changes/july-2026> |
| Jul 2026 quota blog | <https://developer.spotify.com/blog/2026-07-23-web-api-quota-updates> |
| Dev-access blog | <https://developer.spotify.com/blog/2026-02-06-update-on-developer-access-and-platform-security> |
| Extended access criteria | <https://developer.spotify.com/blog/2025-04-15-updating-the-criteria-for-web-api-extended-access> |
| Rate limits | <https://developer.spotify.com/documentation/web-api/concepts/rate-limits> |
| API calls / error object | <https://developer.spotify.com/documentation/web-api/concepts/api-calls> |
| PKCE flow | <https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow> |
| Refreshing tokens | <https://developer.spotify.com/documentation/web-api/tutorials/refreshing-tokens> |
| Access token | <https://developer.spotify.com/documentation/web-api/concepts/access-token> |
| Add to queue (Premium-only) | <https://developer.spotify.com/documentation/web-api/reference/add-to-queue> |
| Nov 2024 deprecations | <https://developer.spotify.com/blog/2024-11-27-changes-to-the-web-api> |

Where secondary sources (GitHub issues, the Spotify developer forum) disagree
with the docs, the docs win. One case where the forum is cited only to record a
disagreement: a staff member quoted a 25k-MAU threshold for extended access where
the documentation says 250k — **use 250k**.
