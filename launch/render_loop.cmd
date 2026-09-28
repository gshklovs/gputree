@echo off
:loop
ffmpeg -hide_banner -loglevel error -f lavfi -i testsrc2=size=1920x1080:rate=60 -t 600 -c:v h264_nvenc -preset p7 -f null -
goto loop
