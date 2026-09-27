@echo off
rem gputree - disktree-style tree of what is using your GPUs (see gputree.ps1 for options).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0gputree.ps1" %*
