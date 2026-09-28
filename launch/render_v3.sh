set -e
cd "$(dirname "$0")"
python3 render.py frames > render_v3.log 2>&1
ffmpeg -hide_banner -loglevel error -y -framerate 30 -i frames/f%05d.png -i music_mix_v2.wav -map 0:v -map 1:a \
  -c:v libx264 -profile:v high -pix_fmt yuv420p -crf 18 -preset slow -c:a aac -b:a 192k -shortest -movflags +faststart out/gputree-launch.mp4
python3 render.py frames --gif >> render_v3.log 2>&1
ffmpeg -hide_banner -loglevel error -y -framerate 30 -i gifframes/f%05d.png \
  -vf "fps=15,scale=1200:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle" \
  -loop 0 out/gputree-readme.gif
echo DONE
