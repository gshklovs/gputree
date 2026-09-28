import librosa, glob, numpy as np, sys
for f in sorted(glob.glob("music/*.mp3")):
    y, sr = librosa.load(f, sr=22050, mono=True, duration=120)
    tempo, beats = librosa.beat.beat_track(y=y, sr=sr)
    t = float(np.atleast_1d(tempo)[0])
    print(f"{t:6.1f}  {len(y)/sr:5.0f}s  {f}")
