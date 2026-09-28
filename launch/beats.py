import librosa, numpy as np, json, sys
f = sys.argv[1]
y, sr = librosa.load(f, sr=22050, mono=True)
tempo, beats = librosa.beat.beat_track(y=y, sr=sr, units="time")
bt = np.array(beats)
d = np.diff(bt)
print("dur", len(y)/sr, "tempo", np.atleast_1d(tempo)[0], "median ibi", np.median(d), "std ibi", d.std())
rms = librosa.feature.rms(y=y, hop_length=2205)[0]  # 0.1 s
for s in range(0, int(len(y)/sr), 8):
    seg = rms[s*10:(s+8)*10]
    print(f"{s:4d}s rms {seg.mean():.3f}")
onset = librosa.onset.onset_strength(y=y, sr=sr)
json.dump({"tempo": float(np.atleast_1d(tempo)[0]), "beats": bt.tolist()}, open(f + ".beats.json", "w"))
