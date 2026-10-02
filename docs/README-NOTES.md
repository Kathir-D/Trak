# README notes

Step 1 of `TODO.md` task 11.1: what was borrowed for the README, and from where, so the next pass on
`README.md` does not have to re-survey. The style reference is a list of *profile* READMEs, which
sell a person; Trak's README sells a tool someone has to install, so only presentation ideas carry
over. The sibling READMEs and two TUI project READMEs are the closer models.

## What works

- **[Kathir-D/Sonar](https://github.com/Kathir-D/Sonar)** (owner's house style): centered icon +
  `<h1>` + one bold pitch line, a `·`-separated anchor row inside the hero, then a separate centered
  badge row with `alt` on every badge. A "Latest release / Install / Requires / Cost" table puts
  the four questions a visitor has above the fold. Long troubleshooting entries each sit in their
  own `<details>`. *Trak:* keep the hero shape it already has; add the at-a-glance table once a
  formula exists; move troubleshooting into per-problem `<details>`.
- **[Kathir-D/headless-spotify](https://github.com/Kathir-D/headless-spotify)**: a nested Contents
  list, a "What it will not do to your Spotify" section, and a Companions section with a stated
  contract (`status --json`). *Trak:* the "polite" promises (never launches Spotify, writes only on
  a key) deserve the same blunt heading; the Sonar/headless section should link the contract in
  `docs/COMPAT.md` rather than restate it.
- **[Rigellute/spotify-tui](https://github.com/Rigellute/spotify-tui)**: one-line pitch, badges,
  then the demo GIF *before any prose*, with a one-line caption naming the terminal theme. Install
  methods go easiest first, from-source last. The Premium/streaming limitation is stated early.
  *Trak:* demo directly under the badge row with a caption naming terminal and font; Homebrew
  first, `cargo build` last; the "unverified" status note stays near the top.
- **[aome510/spotify-player](https://github.com/aome510/spotify-player)**: commands as a
  three-column table (action, description, default key); per-protocol image examples (iTerm2,
  Kitty, Sixel) each with alt text; long config deferred to a separate doc with one link.
  *Trak:* the key table should be action / key / note; show art in one graphics protocol and the
  half-block fallback so the claim "works without graphics" is visible; the full settings list
  can stay in `<details>` or point at `docs/SPEC.md`.
- **[ratatui/ratatui](https://github.com/ratatui/ratatui)**: badge rows grouped by meaning
  (project facts, then CI/health) plus a centered quick-links row; the table of contents is itself
  a `<details>`. *Trak:* two badge rows at most (facts: macOS, Rust, license, release; health: CI);
  keep the contents line visible, since Trak's is short.
- **[caneco/caneco](https://github.com/caneco/caneco)** (Minimalistic): about 100 words, no images,
  emoji used as visual rhythm rather than decoration. *Trak:* the "Why Trak" block stays three
  bullets; one icon per feature row, not per sentence.
- **[filiptronicek/filiptronicek](https://github.com/filiptronicek/filiptronicek)** (Descriptive):
  short greeting, then everything long behind `<details>`, with tables for structured facts.
  *Trak:* progressive disclosure for the CLI reference and settings so the page scrolls to Credits
  in a few screens.
- **[br3ndonland/br3ndonland](https://github.com/br3ndonland/br3ndonland)** (Badges): one shields
  style (`flat-square`) and `logo=` throughout, colours chosen by meaning, every badge a link.
  *Trak:* one style for all badges; link each (macOS → Requirements, license → `LICENSE`).
- **[terrytangyuan/terrytangyuan](https://github.com/terrytangyuan/terrytangyuan)** (Icons): a
  single centered row of local SVGs, each with platform-name `alt`. *Trak:* if a wordmark or icon is
  added, commit it under `docs/images/` rather than hot-linking.

## Applied to Trak

- [ ] Hero: wordmark/icon + bold pitch + anchor row (Sonar), centered.
- [ ] Badges: one style, linked, alt text; add release and Homebrew badges only once they exist.
- [ ] Demo GIF/PNG under the badges, captioned, `alt` set, < ~3 MB, in `docs/images/` (spotify-tui).
      Check it on GitHub in light and dark mode; use `<picture>` only if a variant is needed.
- [ ] At-a-glance table: release / install / requires / cost (Sonar), after the formula ships.
- [ ] Why Trak: three bullets; Features: one icon per row (caneco).
- [ ] Install: Homebrew first, source last (spotify-tui).
- [ ] Keys: action / key / note table (spotify-player); settings and CLI in `<details>`.
- [ ] Art: one protocol screenshot + half-block fallback (spotify-player).
- [ ] "What Trak will not do to Spotify" + Sonar/headless section linking `docs/COMPAT.md`.
- [ ] Troubleshooting: one `<details>` per symptom, permissions first (Sonar).
- [ ] Credits and License last.

## Not borrowed

- Stats cards, visitor counters, streak and "now playing" widgets (anuraghazra, novatorem): they
  hot-link a third-party server, break when it is down, and say nothing about the tool.
- Decorative banners and GIFs (saadeghi's centered logo image, which also has no `alt`): an image
  that is not the product is noise; the only picture above the fold should be Trak itself.
- Social/contact icon rows, skills grids, star-history and "follow me" buttons.
- Badges as biography (br3ndonland's twenty-plus): a project badge must state a fact a user needs.
- Contributor emoji tables (spotify-tui): one maintainer; the Credits section already names upstream.
