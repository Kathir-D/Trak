on run argv
	set U to ASCII character 31
	set s to ""
	tell application "Spotify"
		set theTrack to current track
		if theTrack is missing value then return "no-track" & U & "0"
		set s to s & (player state as string) & U
		set s to s & ((player position) as string) & U
		set s to s & ((sound volume) as string) & U
		set s to s & ((shuffling) as string) & U
		set s to s & ((repeating) as string) & U
		set s to s & (name of theTrack) & U
		set s to s & (artist of theTrack) & U
		set s to s & (album of theTrack) & U
		set s to s & (album artist of theTrack) & U
		set s to s & ((duration of theTrack) as string) & U
		set s to s & ((disc number of theTrack) as string) & U
		set s to s & ((track number of theTrack) as string) & U
		set s to s & ((popularity of theTrack) as string) & U
		set s to s & ((played count of theTrack) as string) & U
		set s to s & (artwork url of theTrack) & U
		set s to s & (spotify url of theTrack) & U
		set s to s & (id of theTrack)
	end tell
	return s
end run
