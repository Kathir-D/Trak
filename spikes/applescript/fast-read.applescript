tell application "Spotify"
	set s to (player state as string) & ASCII character 31 & ((player position) as string)
	set s to s & ASCII character 31 & ((sound volume) as string)
	set s to s & ASCII character 31 & ((shuffling) as string)
	set s to s & ASCII character 31 & ((repeating) as string)
	set s to s & ASCII character 31 & (id of current track)
	return s
end tell
