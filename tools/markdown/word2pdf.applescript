on run argv
	set inPath to item 1 of argv
	set outPath to item 2 of argv
	tell application "Microsoft Word"
		activate
		set display alerts to alerts none
		open file name inPath
		set waited to 0
		repeat while (count of documents) = 0
			delay 0.5
			set waited to waited + 1
			if waited > 60 then error "Word did not open " & inPath
		end repeat
		set d to document 1
		save as d file name outPath file format format PDF
		close d saving no
	end tell
	return "ok"
end run
