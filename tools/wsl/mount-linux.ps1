<#
  Montuje w WSL partycje Linuksa z dysków tego komputera (dual boot: np. CachyOS obok Windows),
  żeby w WSL używać tego samego repozytorium i cache buildów co w Linuksie.

  - wyszukuje dyski z partycjami Linuksa (GPT: Linux filesystem/root/home/LUKS/LVM, MBR: 0x83/0x8e),
  - rozpoznaje system plików partycji (ext4, btrfs, xfs, f2fs) i montuje je przez sam WSL
    (wsl --mount --partition) w /mnt/wsl/PHYSICALDRIVE<n>p<m> — ten katalog widzą wszystkie sesje
    i dystrybucje WSL; dla wygody /mnt/linux/<etykieta> to dowiązania do nich,
  - pokazuje katalogi domowe i znalezione repozytoria videopipeline.

  Użycie (uruchamia się samo jako administrator):
    mount-linux.cmd                 montowanie (domyślna dystrybucja WSL)
    mount-linux.cmd -Distro Debian  wskazana dystrybucja
    mount-linux.cmd -Unmount        odmontowanie i odłączenie dysków

  Montowanie znika po "wsl --shutdown" i restarcie Windows — wtedy uruchom skrypt ponownie.
  Linux na tym dysku musi być normalnie zamknięty (nie hibernacja).
#>
param([string]$Distro = '', [switch]$Unmount)
# 'Continue': w Windows PowerShell 5.1 przekierowanie stderr programu (2>&1) przy 'Stop' kończy skrypt.
$ErrorActionPreference = 'Continue'

# Administrator jest wymagany przez wsl --mount.
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    $a = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-NoExit', '-File', "`"$PSCommandPath`"")
    if ($Distro) { $a += @('-Distro', $Distro) }
    if ($Unmount) { $a += '-Unmount' }
    Start-Process powershell -Verb RunAs -ArgumentList $a
    exit
}

$wslArgs = @()
if ($Distro) { $wslArgs = @('-d', $Distro) }

# Skrypt dla strony Linuksa (obok tego pliku); kopia bez CR na wypadek końców linii Windows.
$shWin = Join-Path $PSScriptRoot 'mount-linux.sh'
if (-not (Test-Path $shWin)) { Write-Host "Brak $shWin"; exit 1 }
$shWsl = (wsl.exe @wslArgs -e wslpath -a "$shWin").Trim()
$uid = (wsl.exe @wslArgs -e id -u).Trim()
function Invoke-LinuxSide([string]$mode) {
    wsl.exe @wslArgs -u root -e bash -c "sed 's/\r$//' '$shWsl' > /tmp/vp-mount-linux.sh && VP_UID=$uid bash /tmp/vp-mount-linux.sh $mode"
}

if ($Unmount) {
    Invoke-LinuxSide 'unlink'
    wsl.exe --unmount | Out-Host
    Write-Host 'Dyski odłączone od WSL.'
    exit 0
}

$linuxTypes = @(
    '{0fc63daf-8483-4772-8e79-3d69d8477de4}', # Linux filesystem
    '{4f68bce3-e8cd-4db1-96e7-fbcaf984b709}', # Linux root (x86-64)
    '{933ac7e1-2eb4-4f13-b844-0e14e2aef915}', # Linux /home
    '{3b8f8425-20e0-4f3b-907f-1a25a76f98e8}', # Linux /srv
    '{ca7d7ccb-63ed-4c53-861c-1742536059cc}', # LUKS
    '{e6d6d379-f507-44c2-a23c-238f2a3df928}'  # LVM
)

# Partycje Windows (GUID) → (dysk, numer partycji); GUID partycji GPT = PARTUUID w Linuksie.
function Get-PartUuid($p) { if ($p.Guid) { $p.Guid.Trim('{}').ToLower() } else { $null } }

$disks = @()
foreach ($disk in Get-Disk | Sort-Object Number) {
    $parts = @(Get-Partition -DiskNumber $disk.Number -ErrorAction SilentlyContinue)
    $linux = @($parts | Where-Object {
        ($_.GptType -and ($linuxTypes -contains $_.GptType.ToLower())) -or $_.MbrType -eq 0x83 -or $_.MbrType -eq 0x8e
    })
    if ($linux.Count -eq 0) { continue }

    $size = [math]::Round($disk.Size / 1GB)
    $drive = "\\.\PHYSICALDRIVE$($disk.Number)"
    Write-Host ''
    Write-Host "Dysk $($disk.Number): $($disk.FriendlyName) ($size GB) — partycje Linuksa: $(($linux | ForEach-Object { $_.PartitionNumber }) -join ', ')"
    if ($disk.IsSystem -or $disk.IsBoot) {
        Write-Host '  Na tym dysku jest Windows — WSL nie może go podłączyć (to ograniczenie wsl --mount). Pomijam.'
        continue
    }
    # WSL przejmuje cały dysk: woluminy Windows z tego dysku znikną na czas montowania.
    $letters = @($parts | Where-Object { $_.DriveLetter } | ForEach-Object { "$($_.DriveLetter):" })
    if ($letters.Count -gt 0) {
        Write-Host "  UWAGA: dysk zawiera też $($letters -join ', ') — na czas montowania w WSL te dyski znikną z Windows."
        $ans = Read-Host '  Podłączyć mimo to? [t/N]'
        if ($ans -notmatch '^[tTyY]') { Write-Host '  Pominięto.'; continue }
    }
    # Najpierw bez montowania (--bare), żeby rozpoznać systemy plików w WSL.
    wsl.exe --unmount $drive 2>&1 | Out-Null
    $out = (wsl.exe --mount $drive --bare 2>&1 | Out-String) -replace "`0", ''
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  Nie udało się podłączyć: $($out.Trim())"
        continue
    }
    $disks += [pscustomobject]@{ Disk = $disk; Drive = $drive; Linux = $linux }
}

if ($disks.Count -eq 0) {
    Write-Host ''
    Write-Host 'Nie podłączono żadnego dysku z Linuksem.'
    exit 1
}

# PARTUUID|FSTYPE|LABEL wszystkich partycji widocznych w WSL.
$found = @{}
foreach ($line in (Invoke-LinuxSide 'detect')) {
    $f = "$line".Split('|')
    if ($f.Count -ge 2 -and $f[0]) { $found[$f[0].ToLower()] = $f }
}

$supported = @('ext4', 'ext3', 'ext2', 'btrfs', 'xfs', 'f2fs')
$links = @()
Write-Host ''
foreach ($d in $disks) {
    # Montowanie partycji przez WSL wymaga zwolnienia dysku podłączonego w trybie --bare.
    wsl.exe --unmount $d.Drive 2>&1 | Out-Null
    foreach ($p in $d.Linux) {
        $uuid = Get-PartUuid $p
        $info = if ($uuid) { $found[$uuid] } else { $null }
        $tag = "dysk $($d.Disk.Number), partycja $($p.PartitionNumber)"
        if (-not $info) { Write-Host "  ${tag}: nie rozpoznano systemu plików — pomijam"; continue }
        $fs = $info[1]; $label = if ($info.Count -ge 3) { $info[2] } else { '' }
        if ($fs -eq 'crypto_LUKS') {
            Write-Host "  ${tag}: szyfrowana (LUKS) — automatyczne montowanie nieobsługiwane, pomijam"
            continue
        }
        if ($supported -notcontains $fs) { Write-Host "  ${tag}: $fs — pomijam"; continue }
        $wslArgsMount = @('--mount', $d.Drive, '--partition', "$($p.PartitionNumber)", '--type', $fs)
        # btrfs: główny poziom (subvolid=5) — widać wszystkie podwolumeny (@, @home…).
        if ($fs -eq 'btrfs') { $wslArgsMount += @('--options', 'subvolid=5,noatime') } else { $wslArgsMount += @('--options', 'noatime') }
        $out = (wsl.exe @wslArgsMount 2>&1 | Out-String) -replace "`0", ''
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  ${tag} ($fs): montowanie nie powiodło się: $($out.Trim())"
            Write-Host '    (Linux zamknięty normalnie, bez hibernacji?)'
            continue
        }
        $target = "/mnt/wsl/PHYSICALDRIVE$($d.Disk.Number)p$($p.PartitionNumber)"
        $name = if ($label) { $label } else { "dysk$($d.Disk.Number)-p$($p.PartitionNumber)" }
        Write-Host "  ${tag} ($fs$(if ($label) { ", $label" })) → $target"
        $links += "$name=$target"
    }
}
if ($links.Count -eq 0) { Write-Host 'Nic nie zamontowano.'; exit 1 }
Write-Host ''
Invoke-LinuxSide ('links ' + (($links | ForEach-Object { "'$_'" }) -join ' '))
