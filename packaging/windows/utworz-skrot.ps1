# Tworzy skroty "VideoPipeline" (pulpit + menu Start) do videopipeline.exe z tego folderu.
# Skrot otwiera program w dolaczonym terminalu videopipeline-term.exe (ikona VideoPipeline na pasku
# zadan i w ramce okna). Bez niego - w klasycznej konsoli conhost.exe (tez z ikona programu).
$ErrorActionPreference = 'Stop'
$dir = $PSScriptRoot
$exe = Join-Path $dir 'videopipeline.exe'
if (-not (Test-Path $exe)) { Write-Host "Brak $exe - uruchom skrypt z folderu VideoPipeline."; exit 1 }
$term = Join-Path $dir 'videopipeline-term.exe'
$conf = Join-Path $dir 'videopipeline-term.toml'
if (Test-Path $term) {
    $target = $term
    $arguments = '--config-file "' + $conf + '" -e "' + $exe + '"'
} else {
    $target = Join-Path $env:SystemRoot 'System32\conhost.exe'
    $arguments = '"' + $exe + '"'
}
$shell = New-Object -ComObject WScript.Shell
$places = @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('Programs'))
foreach ($place in $places) {
    $lnk = $shell.CreateShortcut((Join-Path $place 'VideoPipeline.lnk'))
    $lnk.TargetPath = $target
    $lnk.Arguments = $arguments
    $lnk.WorkingDirectory = [Environment]::GetFolderPath('MyVideos')
    $lnk.IconLocation = "$exe,0"
    $lnk.Description = 'VideoPipeline - kodowanie wideo na GPU'
    $lnk.Save()
    Write-Host "Utworzono skrot: $($lnk.FullName)"
}
Write-Host 'Gotowe. Skrot mozna tez przypiac do paska zadan (prawy przycisk -> Przypnij do paska zadan).'
