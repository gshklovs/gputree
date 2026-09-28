import re, subprocess
M = "music/lazy-summer-lofi-1__windows-downmp3.mp3"
W = "min(max((t-12.0)/0.3,0),1)*(1-min(max((t-15.28)/0.4,0),1))"
FC = (f"[0:a]atrim=85.12:109.12,asetpts=PTS-STARTPTS,volume=0.6,asplit[a][b];[b]lowpass=f=450,lowpass=f=450,volume=1.6[l];"
      f"[a]volume='1-{W}':eval=frame[a2];[l]volume='{W}':eval=frame[l2];[a2][l2]amix=inputs=2:normalize=0,"
      f"alimiter=limit=0.5:level=false,afade=t=in:d=0.25,afade=t=out:st=22.7:d=1.3")
r = subprocess.run(["ffmpeg", "-hide_banner", "-y", "-i", M, "-filter_complex", FC + ",loudnorm=I=-14:TP=-1.5:LRA=11:print_format=json", "-f", "null", "-"], capture_output=True, text=True)
g = lambda k: re.search(rf'"{k}"\s*:\s*"([^"]+)"', r.stderr).group(1)
fc2 = FC + (f",loudnorm=I=-14:TP=-1.5:LRA=11:measured_I={g('input_i')}:measured_TP={g('input_tp')}:measured_LRA={g('input_lra')}"
            f":measured_thresh={g('input_thresh')}:offset={g('target_offset')}:linear=true:print_format=summary,aresample=48000")
r2 = subprocess.run(["ffmpeg", "-hide_banner", "-y", "-i", M, "-filter_complex", fc2, "-ar", "48000", "-ac", "2", "music_mix.wav"], capture_output=True, text=True)
print(g('input_i'), g('input_tp')); print(r2.stderr[-600:])
