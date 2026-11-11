<#
  Montuje w WSL partycje Linuksa z dysków tego komputera (dual boot: np. CachyOS obok Windows),
  żeby w WSL używać tego samego repozytorium i cache buildów co w Linuksie.

  - wyszukuje dyski z partycjami Linuksa (GPT: Linux filesystem/root/home/LUKS/LVM, MBR: 0x83/0x8e),
  - podłącza je do WSL (wsl --mount --bare), a w WSL rozpoznaje system plików (ext4, btrfs, xfs, f2fs,
    LUKS — po podaniu hasła) i montuje każdą partycję w /mnt/linux/<etykieta>,
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
    Invoke-LinuxSide 'unmount'
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

$attached = 0
foreach ($disk in Get-Disk | Sort-Object Number) {
    $parts = @(Get-Partition -DiskNumber $disk.Number -ErrorAction SilentlyContinue)
    $linux = @($parts | Where-Object {
        ($_.GptType -and ($linuxTypes -contains $_.GptType.ToLower())) -or $_.MbrType -eq 0x83 -or $_.MbrType -eq 0x8e
    })
    if ($linux.Count -eq 0) { continue }

    $size = [math]::Round($disk.Size / 1GB)
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
    $out = wsl.exe --mount "\\.\PHYSICALDRIVE$($disk.Number)" --bare 2>&1 | Out-String
    $out = $out -replace "`0", ''
    if ($LASTEXITCODE -ne 0 -and $out -notmatch 'already|już') {
        Write-Host "  Nie udało się podłączyć: $($out.Trim())"
        continue
    }
    Write-Host '  Podłączony do WSL.'
    $attached++
}

if ($attached -eq 0) {
    Write-Host ''
    Write-Host 'Nie podłączono żadnego dysku z Linuksem.'
    exit 1
}
Write-Host ''
Invoke-LinuxSide 'mount'
