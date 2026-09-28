**gputree: nvidia-smi, but it tells you what's actually running.**

A disktree-style tree of what is using your GPUs on Windows: adapter → process → engine, and for WSL2 the Linux processes inside the VM. On top is one plain-English sentence, picked by Jev (TypeSafe's typed-decision model), with a local-rules fallback when Jev is unreachable. The same workspace also builds **cputree**: the whole process tree with CPU rolled up.

- `gputree.exe`: GPU tree (`gputree -w .5` for live mode)
- `cputree.exe`: CPU process tree
- `gputree-launch.mp4`: 24 s launch video. Every number in it comes from real captured output.

Windows 10/11 x64. Rust, reading Windows' native performance counters: first frame in about 21 ms. On a terminal the tree folds to fit the screen, and the headline's key phrases are coloured.

Music in the video: "Windows Down" by HoliznaCC0, CC0 1.0 (see `assets/launch/MUSIC-LICENSE.txt`).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
