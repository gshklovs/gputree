<#
gputree - a disktree-style tree of what is using your GPUs. Read-only: never touches the processes.

usage: gputree [-Metric vram|util] [-Depth N] [-Watch SECONDS] [-Top N] [-Group] [-All] [-NoWsl]

  GPU adapter -> Windows process -> engines (3D / Copy / Compute / Video...)
                                  -> for the WSL2 VM (vmwp): the Linux processes holding /dev/dxg
Largest first. Bars are relative to the parent (adapter VRAM for memory, 100% for util).

  -Metric  rank by vram (default) or util
  -Depth   levels under each adapter (1-3, default 3)
  -Watch   redraw every N seconds until Ctrl+C
  -Group   insert a tag layer: adapter -> tag (training, game, video render...) -> process
  -Top     processes shown per adapter (default 10; -All lifts it)
  -All     list every process with a GPU handle, including idle ones
  -NoWsl   skip looking inside WSL distros
#>
param(
  [ValidateSet('vram', 'util')][string]$Metric = 'vram',
  [ValidateRange(1, 3)][int]$Depth = 3,
  [double]$Watch = 0,
  [switch]$All,
  [int]$Top = 10,
  [switch]$Group,
  [switch]$NoWsl
)

$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$E = [char]27
$C = @{ dim = "$E[2m"; bold = "$E[1m"; r = "$E[0m"; cyan = "$E[36m"; green = "$E[32m"; yellow = "$E[33m"; red = "$E[31m"; mag = "$E[35m" }

function Fmt-Bytes([double]$b) {
  if ($b -ge 1GB) { return '{0:N1} GiB' -f ($b / 1GB) }
  if ($b -ge 1MB) { return '{0:N0} MiB' -f ($b / 1MB) }
  if ($b -ge 1KB) { return '{0:N0} KiB' -f ($b / 1KB) }
  return '{0:N0} B' -f $b
}

function Bar([double]$frac, [int]$w = 16) {
  $frac = [math]::Max([double]0, [math]::Min([double]1, $frac))  # int literals would pick the int overload and round to 0
  $n = [math]::Round($frac * $w)
  $col = if ($frac -ge 0.66) { $C.red } elseif ($frac -ge 0.33) { $C.yellow } else { $C.green }
  return "$col$([string]([char]0x2588) * $n)$($C.dim)$([string]([char]0x2591) * ($w - $n))$($C.r)"
}

# "0x00000000_0x000134ee" -> 0x134ee as a comparable key
function Luid-Key([string]$s) { $h, $l = $s -split '_'; return ([uint64]$h * 4294967296 + [uint64]$l) }

# What kind of workload is this? name/path/cmdline rules first, then GPU-engine hints.
$TagColor = @{
  'training' = "$E[1;31m"; 'ai inference' = "$E[1;35m"; 'compute' = "$E[35m"; 'video render' = "$E[1;33m"; 'recording' = "$E[33m"
  'video playback' = "$E[33m"; 'game' = "$E[1;32m"; 'game?' = "$E[32m"; 'launcher' = "$E[32m"; '3d / cad' = "$E[1;34m"; 'game dev' = "$E[1;34m"
  'browser' = "$E[36m"; 'app' = "$E[37m"; 'terminal' = "$E[37m"; 'desktop' = "$E[2m"; 'vm' = "$E[35m"; 'audio' = "$E[2m"; 'system' = "$E[2m"; 'other' = "$E[2m"
}
function Get-Tag([string]$name, [string]$path, [string]$cmd, $eng) {
  $n = $name.ToLower(); $p = "$path".ToLower(); $c = "$cmd".ToLower()
  $py = $c -match '(^|/|\s)python[0-9.]*\s' -or $n -like 'python*'
  if ($c -match '\btrain(ing)?\b|rsl_rl|isaac|torchrun|deepspeed|accelerate launch|lightning|\bppo\b|finetune|fine_tune') { return 'training' }
  if ("$n $c" -match 'ollama|llama-server|llama\.cpp|vllm|comfyui|stable.?diffusion|lm ?studio|koboldcpp|whisper|text-generation|sglang') { return 'ai inference' }
  if ($py -or $n -match '^(jupyter|julia|matlab)') { return 'compute' }
  if ($n -match '^(ffmpeg|handbrake|adobe premiere pro|premiere|resolve|afterfx|vegas\d*|topaz|shotcut|kdenlive|adobe media encoder|capcut|clipchamp)') { return 'video render' }
  if ($n -match '^(obs\d*|streamlabs|nvidia share|gamebar)') { return 'recording' }
  if ($n -match '^(blender|maya|3dsmax|cinema 4d|houdini|fusion360|freecad|solidworks|sketchup|rhino|unrealeditor|unity|substance)') { return '3d / cad' }
  if ($n -match '^(godot|megadot)') { return 'game dev' }
  if ($p -match 'steamapps\\common|\\epic games\\|\\xboxgames\\|\\riot games\\|battle\.net|\\ea games\\|\\ubisoft\\|\\gog games\\|\\games\\') { return 'game' }
  if ($n -match '^(steam|steamwebhelper|epicgameslauncher|battle\.net|riotclientservices|eadesktop|galaxyclient|xboxpcapp)$') { return 'launcher' }
  if ($n -match '^(vlc|mpv|mpc-hc\d*|video\.ui|microsoft\.media\.player|netflix|potplayer)') { return 'video playback' }
  if ($n -match '^(chrome|msedge|firefox|brave|opera|arc|vivaldi|zen)$') { return 'browser' }
  if ($n -match '^(windowsterminal|conhost|wezterm.*|alacritty|openconsole)$') { return 'terminal' }
  if ($n -match '^(vmwp|vmmem.*)$') { return 'vm' }
  if ($n -eq 'audiodg') { return 'audio' }
  if ($n -in 'system', 'idle', 'registry') { return 'system' }
  if ($n -match '^(dwm|explorer|csrss|system|shellhost|startmenuexperiencehost|searchhost|textinputhost|applicationframehost|widgetboard|widgets|lockapp|shellexperiencehost|crossdeviceresume|powertoys.*|microsoft\.cmdpal\.ui|phoneexperiencehost|systemsettings)$') { return 'desktop' }
  if ($n -match '^(discord|slack|teams|ms-teams|zoom|claude|code|cursor|spotify|m365copilot|mscopilot|whatsapp|notion|obsidian|msedgewebview2|onedrive|outlook|olk)$') { return 'app' }
  if ($eng) {
    if ([double]$eng['videoencode'] -ge 1) { return 'video render' }
    if ([double]$eng['videodecode'] -ge 1) { return 'video playback' }
    if ([double]$eng['compute'] -ge 1) { return 'compute' }
    if ([double]$eng['3d'] -ge 20 -and $p -and $p -notmatch '\\windows\\|\\windowsapps\\') { return 'game?' }
  }
  return 'other'
}
function Tag-Chip([string]$t) { "$($TagColor[$t])[$t]$($C.r)" }

function Get-Adapters {
  $map = @{}
  foreach ($k in Get-ChildItem HKLM:\SOFTWARE\Microsoft\DirectX) {
    $p = Get-ItemProperty $k.PSPath
    if ($p.Description -and $p.AdapterLuid) {
      $map[[uint64]$p.AdapterLuid] = [pscustomobject]@{ Name = $p.Description; Total = [double]$p.DedicatedVideoMemory; Shared = [double]$p.SharedSystemMemory }
    }
  }
  return $map
}

function Get-NvidiaStats {
  $smi = Get-Command nvidia-smi -ErrorAction SilentlyContinue
  if (-not $smi) { return $null }
  $line = & $smi.Source --query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw --format=csv,noheader,nounits 2>$null | Select-Object -First 1
  if (-not $line) { return $null }
  $f = $line -split ',\s*'
  return [pscustomobject]@{ Name = $f[0]; Util = [double]$f[1]; Used = [double]$f[2] * 1MB; Total = [double]$f[3] * 1MB; Temp = $f[4]; Power = $f[5] }
}

# Linux processes inside every running WSL distro that have the paravirtual GPU (/dev/dxg) open.
function Get-WslGpuProcs {
  $env:WSL_UTF8 = '1'
  $distros = wsl.exe -l --running -q 2>$null | ForEach-Object { ($_ -replace "`0", '').Trim() } | Where-Object { $_ }
  $sh = @'
for p in /proc/[0-9]*; do
  ls -l $p/fd 2>/dev/null | grep -q /dev/dxg || continue
  pid=${p#/proc/}
  rss=$(awk '/^VmRSS/{print $2}' $p/status 2>/dev/null)
  user=$(stat -c %U $p 2>/dev/null)
  cmd=$(tr '\0' ' ' < $p/cmdline 2>/dev/null)
  echo "$pid|${rss:-0}|$user|$cmd"
done
'@
  $b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(($sh -replace "`r", '')))
  $out = @()
  foreach ($d in $distros) {
    $lines = wsl.exe -d $d -u root -- sh -c "echo $b64 | base64 -d | sh" 2>$null
    foreach ($l in $lines) {
      $f = $l -split '\|', 4
      if ($f.Count -lt 4) { continue }
      $out += [pscustomobject]@{ Distro = $d; Pid = [int]$f[0]; Rss = [double]$f[1] * 1KB; User = $f[2]; Cmd = $f[3].Trim() }
    }
  }
  return $out
}

function Collect {
  $paths = '\GPU Engine(*)\Utilization Percentage', '\GPU Process Memory(*)\Dedicated Usage', '\GPU Process Memory(*)\Shared Usage', '\GPU Adapter Memory(*)\Dedicated Usage', '\GPU Adapter Memory(*)\Shared Usage'
  $samples = (Get-Counter $paths -SampleInterval 1 -MaxSamples 1).CounterSamples
  $adapters = Get-Adapters
  $procs = @{}   # "luid|pid" -> record
  $adMem = @{}   # luid -> @{Ded; Shr}
  $rx = '^pid_(\d+)_luid_(0x[0-9a-f]+_0x[0-9a-f]+)_phys_\d+(?:_eng_\d+_engtype_(.*))?$'

  foreach ($s in $samples) {
    $inst = $s.InstanceName.ToLower() -replace '#\d+$', ''
    $ctr = ($s.Path -split '\\')[-1]
    if ($s.Path -match 'gpu adapter memory') {
      if ($inst -match '^luid_(0x[0-9a-f]+_0x[0-9a-f]+)') {
        $k = Luid-Key $Matches[1]
        if (-not $adMem[$k]) { $adMem[$k] = @{ Ded = 0.0; Shr = 0.0 } }
        if ($ctr -eq 'dedicated usage') { $adMem[$k].Ded += $s.CookedValue } else { $adMem[$k].Shr += $s.CookedValue }
      }
      continue
    }
    if ($inst -notmatch $rx) { continue }
    $procId = [int]$Matches[1]; $luid = Luid-Key $Matches[2]; $eng = $Matches[3]
    $key = "$luid|$procId"
    if (-not $procs[$key]) { $procs[$key] = [pscustomobject]@{ Luid = $luid; Pid = $procId; Ded = 0.0; Shr = 0.0; Eng = @{} } }
    $r = $procs[$key]
    switch -Wildcard ($ctr) {
      'utilization percentage' { $t = if ($eng) { $eng -replace '_\d+$', '' } else { '?' }; $r.Eng[$t] = [double]$r.Eng[$t] + $s.CookedValue }
      'dedicated usage' { $r.Ded = [math]::Max($r.Ded, $s.CookedValue) }
      'shared usage' { $r.Shr = [math]::Max($r.Shr, $s.CookedValue) }
    }
  }

  $names = @{}; $exe = @{}
  Get-Process | ForEach-Object { $names[$_.Id] = $_.ProcessName; $exe[$_.Id] = $_.Path }
  foreach ($r in $procs.Values) {
    $r | Add-Member Name ($(if ($names[$r.Pid]) { $names[$r.Pid] } elseif ($r.Pid -eq 4) { 'System' } else { '<exited>' }))
    $r | Add-Member Util ([double](($r.Eng.Values | Measure-Object -Maximum).Maximum))
    $r | Add-Member Tag (Get-Tag $r.Name $exe[$r.Pid] '' $r.Eng)
  }

  $npu = (Get-PnpDevice -Class ComputeAccelerator -PresentOnly | Select-Object -First 1).FriendlyName
  $luids = @($adapters.Keys) + @($adMem.Keys) + @($procs.Values | ForEach-Object Luid) | Sort-Object -Unique
  $nv = Get-NvidiaStats
  $wsl = $null
  $vmBusy = $procs.Values | Where-Object { $_.Name -eq 'vmwp' -and ($_.Util -gt 0 -or $_.Ded -gt 0) }
  if ($vmBusy -and -not $NoWsl) {
    $wsl = @(Get-WslGpuProcs)
    foreach ($w in $wsl) { $w | Add-Member Tag (Get-Tag (($w.Cmd -split '\s')[0] -replace '^.*/', '') '' $w.Cmd $null) }
    # the VM is doing whatever its busiest Linux workload is
    $inner = $wsl | Where-Object { $_.Tag -ne 'other' } | Sort-Object Rss -Descending | Select-Object -First 1
    if ($inner) { foreach ($v in $vmBusy) { $v.Tag = $inner.Tag } }
  }

  $gpus = foreach ($l in $luids) {
    $a = $adapters[$l]
    if ($a -and $a.Name -like 'Microsoft Basic Render*') { continue }
    $mine = @($procs.Values | Where-Object Luid -eq $l)
    $integrated = (-not $a) -or $a.Total -le 512MB
    $mem = if ($adMem[$l]) { if ($integrated) { $adMem[$l].Shr } else { $adMem[$l].Ded } } else { 0 }
    $cap = if ($a) { if ($integrated) { $a.Shared } else { $a.Total } } else { 0 }
    # adapter util = busiest engine type summed across processes (what Task Manager shows)
    $engSum = @{}
    foreach ($r in $mine) { foreach ($kv in $r.Eng.GetEnumerator()) { $engSum[$kv.Key] = [double]$engSum[$kv.Key] + $kv.Value } }
    $util = [double](($engSum.Values | Measure-Object -Maximum).Maximum)
    $extra = $null
    if ($nv -and $a -and $a.Name -like "*$($nv.Name -replace 'NVIDIA ', '')*") { $extra = $nv; $util = [math]::Max($util, $nv.Util) }
    [pscustomobject]@{ Luid = $l; Name = $(if ($a) { $a.Name } elseif ($npu) { "$npu (compute, luid 0x{0:x})" -f $l } else { 'compute device luid 0x{0:x}' -f $l }); Integrated = $integrated; Mem = $mem; Cap = $cap; Util = $util; Eng = $engSum; Procs = $mine; Nv = $extra }
  }
  return [pscustomobject]@{ Gpus = @($gpus | Where-Object { $_.Procs.Count -gt 0 -or $_.Mem -gt 0 }); Wsl = $wsl; Time = Get-Date }
}

function Render($snap) {
  $width = [math]::Max(60, $Host.UI.RawUI.WindowSize.Width - 1)
  $lines = New-Object System.Collections.Generic.List[string]
  $emit = { param($s) $lines.Add($s) }
  $V = [char]0x2502; $T = [char]0x251C; $L = [char]0x2514; $H = [char]0x2500

  & $emit "$($C.bold)gputree$($C.r)$($C.dim)  $($snap.Gpus.Count) adapter(s) - ranked by $Metric - $($snap.Time.ToString('HH:mm:ss'))$($C.r)"
  foreach ($g in $snap.Gpus | Sort-Object @{ e = { if ($Metric -eq 'util') { $_.Util } else { $_.Mem } } } -Descending) {
    & $emit ''
    $memTxt = if ($g.Cap -gt 0) { "$(Fmt-Bytes $g.Mem) / $(Fmt-Bytes $g.Cap)$(if ($g.Integrated) { ' shared' })" } else { Fmt-Bytes $g.Mem }
    $nvTxt = if ($g.Nv) { "  $($g.Nv.Temp)C  $([math]::Round([double]$g.Nv.Power))W" } else { '' }
    & $emit "$($C.bold)$($C.cyan)$($g.Name)$($C.r)"
    $engTxt = ($g.Eng.GetEnumerator() | Where-Object Value -gt 0.05 | Sort-Object Value -Descending | ForEach-Object { '{0} {1:N0}%' -f $_.Key, [math]::Min(100, $_.Value) }) -join '  '
    & $emit "  mem  $(Bar ($(if ($g.Cap) { $g.Mem / $g.Cap } else { 0 })) 24) $memTxt"
    & $emit "  util $(Bar ($g.Util / 100) 24) $('{0:N0}%' -f $g.Util)$nvTxt$($C.dim)  $engTxt$($C.r)"

    $pmem = { param($p) if ($g.Integrated) { $p.Shr } else { $p.Ded } }
    $key1 = @{ e = { if ($Metric -eq 'util') { $_.Util } else { & $pmem $_ } } }
    $key2 = @{ e = { if ($Metric -eq 'util') { & $pmem $_ } else { $_.Util } } }

    # per-tag rollup line
    $byTag = @($g.Procs | Group-Object Tag | ForEach-Object {
      [pscustomobject]@{ Tag = $_.Name; Mem = [double](($_.Group | ForEach-Object { & $pmem $_ } | Measure-Object -Sum).Sum); Util = [double](($_.Group | Measure-Object Util -Sum).Sum); Procs = @($_.Group) }
    } | Sort-Object @{ e = { if ($Metric -eq 'util') { $_.Util } else { $_.Mem } } }, @{ e = { if ($Metric -eq 'util') { $_.Mem } else { $_.Util } } } -Descending)
    $tagTxt = ($byTag | Where-Object { $_.Mem -ge 1MB -or $_.Util -ge 0.5 } | Select-Object -First 6 | ForEach-Object { "$(Tag-Chip $_.Tag) $(Fmt-Bytes $_.Mem) $('{0:N0}%' -f $_.Util)" }) -join '  '
    if ($tagTxt) { & $emit "  tags $tagTxt" }

    $emitProc = {
      param($p, $prefix, $last)
      $br = if ($last) { "$L$H " } else { "$T$H " }
      $pad = $prefix + $(if ($last) { '   ' } else { "$V  " })
      $pm = & $pmem $p
      $frac = if ($Metric -eq 'util') { $p.Util / 100 } elseif ($g.Mem -gt 0) { $pm / $g.Mem } else { 0 }
      $label = if ($p.Name -eq 'vmwp') { "vmwp $($C.mag)(WSL2 VM)$($C.r)" } else { $p.Name }
      $chip = if ($Group) { '' } else { ' ' + (Tag-Chip $p.Tag) }
      & $emit ("{0}{1}{2} {3,9} {4,5} {5}{6}{7}" -f $prefix, $br, (Bar $frac 12), (Fmt-Bytes $pm), ('{0:N0}%' -f $p.Util), $label, $chip, "$($C.dim)  pid $($p.Pid)$($C.r)")
      $kids = New-Object System.Collections.Generic.List[string]
      if ($Depth -ge 3) {
        foreach ($kv in $p.Eng.GetEnumerator() | Where-Object Value -gt 0.05 | Sort-Object Value -Descending) {
          $kids.Add(("{0} {1,-12} {2,5}" -f (Bar ($kv.Value / 100) 8), $kv.Key, ('{0:N1}%' -f $kv.Value)))
        }
      }
      if ($Depth -ge 2 -and $p.Name -eq 'vmwp' -and $snap.Wsl -and ($p.Util -ge 0.5 -or $p.Ded -ge 64MB)) {
        foreach ($w in $snap.Wsl | Sort-Object Rss -Descending) {
          $kids.Add("$($C.mag)$($w.Distro)$($C.r) $(Tag-Chip $w.Tag) $($C.bold)$($w.Cmd)$($C.r)$($C.dim)  pid $($w.Pid) $($w.User) rss $(Fmt-Bytes $w.Rss)$($C.r)")
        }
      }
      if ($Depth -ge 3 -and -not $g.Integrated -and $p.Shr -ge 1MB) { $kids.Add("$($C.dim)+ $(Fmt-Bytes $p.Shr) spilled to shared system memory$($C.r)") }
      for ($j = 0; $j -lt $kids.Count; $j++) {
        $kb = if ($j -eq $kids.Count - 1) { "$L$H " } else { "$T$H " }
        & $emit "$pad$kb$($kids[$j])"
      }
    }

    $isActive = { $All -or $_.Util -ge 0.1 -or ($_.Ded + $_.Shr) -ge 1MB }
    if ($Group) {
      $groups = @($byTag | Where-Object { $All -or $_.Mem -ge 1MB -or $_.Util -ge 0.1 })
      $restN = [int](($byTag | Where-Object { $groups -notcontains $_ } | ForEach-Object { $_.Procs.Count } | Measure-Object -Sum).Sum)
      for ($i = 0; $i -lt $groups.Count; $i++) {
        $tg = $groups[$i]
        $lastT = ($i -eq $groups.Count - 1) -and $restN -le 0
        $frac = if ($Metric -eq 'util') { $tg.Util / 100 } elseif ($g.Mem -gt 0) { $tg.Mem / $g.Mem } else { 0 }
        & $emit ("{0}{1} {2,9} {3,5} {4}{5}" -f $(if ($lastT) { "$L$H " } else { "$T$H " }), (Bar $frac 12), (Fmt-Bytes $tg.Mem), ('{0:N0}%' -f $tg.Util), (Tag-Chip $tg.Tag), "$($C.dim)  $($tg.Procs.Count) process(es)$($C.r)")
        if ($Depth -lt 2) { continue }
        $sub = @($tg.Procs | Where-Object $isActive | Sort-Object $key1, $key2 -Descending)
        if (-not $All -and $sub.Count -gt $Top) { $sub = $sub[0..($Top - 1)] }
        $more = $tg.Procs.Count - $sub.Count
        $pre = if ($lastT) { '   ' } else { "$V  " }
        for ($k = 0; $k -lt $sub.Count; $k++) { & $emitProc $sub[$k] $pre (($k -eq $sub.Count - 1) -and $more -le 0) }
        if ($more -gt 0) { & $emit "$pre$L$H $($C.dim)+ $more more$($C.r)" }
      }
      if ($restN -gt 0) { & $emit "$L$H $($C.dim)+ $restN idle process(es) (-All to list)$($C.r)" }
    } else {
      $sorted = @($g.Procs | Where-Object $isActive | Sort-Object $key1, $key2 -Descending)
      if (-not $All -and $sorted.Count -gt $Top) { $sorted = $sorted[0..($Top - 1)] }
      $hidden = $g.Procs.Count - $sorted.Count
      $hiddenMem = ($g.Procs | Where-Object { $sorted -notcontains $_ } | ForEach-Object { & $pmem $_ } | Measure-Object -Sum).Sum
      for ($i = 0; $i -lt $sorted.Count; $i++) { & $emitProc $sorted[$i] '' (($i -eq $sorted.Count - 1) -and $hidden -le 0) }
      if ($hidden -gt 0) { & $emit "$L$H $($C.dim)+ $hidden more process(es), $(Fmt-Bytes $hiddenMem) combined (-All to list)$($C.r)" }
    }
  }
  if ($snap.Wsl) { & $emit ''; & $emit "$($C.dim)WSL2 shares one VM, so per-Linux-process GPU % is not visible from Windows; the VM total above is theirs combined.$($C.r)" }

  # clip to terminal width (ANSI-aware enough: only truncate plain overflow)
  foreach ($ln in $lines) {
    $plain = $ln -replace "$E\[[0-9;]*m", ''
    if ($plain.Length -gt $width) {
      $over = $plain.Length - $width + 1
      $ln = $ln.Substring(0, [math]::Max(0, $ln.Length - $over)) + "$($C.r)" + [char]0x2026
    }
    $ln
  }
}

if ($Watch -gt 0) {
  try {
    [Console]::CursorVisible = $false
    while ($true) {
      $out = Render (Collect)
      [Console]::Write("$E[H$E[2J" + ($out -join "`n") + "`n$($C.dim)refresh ${Watch}s - Ctrl+C to quit$($C.r)")
      Start-Sleep -Milliseconds ([math]::Max(0, $Watch * 1000 - 1000))
    }
  } finally { [Console]::CursorVisible = $true; Write-Host '' }
} else {
  Render (Collect) | ForEach-Object { Write-Host $_ }
}
