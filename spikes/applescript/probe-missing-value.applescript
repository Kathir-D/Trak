on run argv
	set U to ASCII character 31
	set o to ""
	tell application "Spotify"
		set t to current track
		try
			set v to name of t
			if v is missing value then set o to o & "name=<missing>" & U
			if v is "" then set o to o & "name=<EMPTY-STRING>" & U
			if v is not "" and v is not missing value then set o to o & "name=<text:" & v & ">" & U
		on error errm
			set o to o & "name=<ERR " & errm & ">" & U
		end try
		try
			set v to artwork url of t
			if v is missing value then set o to o & "artwork=<missing>" & U
			if v is "" then set o to o & "artwork=<EMPTY>" & U
			if v is not "" and v is not missing value then set o to o & "artwork=<text:" & v & ">" & U
		on error errm
			set o to o & "artwork=<ERR " & errm & ">" & U
		end try
		try
			set v to album of t
			if v is "" then set o to o & "album=<EMPTY>" & U
			if v is not missing value and v is not "" then set o to o & "album=<text:" & v & ">" & U
		on error errm
			set o to o & "album=<ERR " & errm & ">" & U
		end try
	end tell
	return o
end run
