# Tworzy skroty "VideoPipeline" (pulpit + menu Start) do videopipeline.exe z tego folderu.
# Skrot uruchamia program przez conhost.exe (klasyczna konsola Windows): w Windows Terminal
# pasek zadan zawsze pokazuje ikone terminala, a w conhost - ikone programu.
$ErrorActionPreference = 'Stop'
$dir = $PSScriptRoot
$exe = Join-Path $dir 'videopipeline.exe'
if (-not (Test-Path $exe)) { Write-Host "Brak $exe - uruchom skrypt z folderu VideoPipeline."; exit 1 }
$conhost = Join-Path $env:SystemRoot 'System32\conhost.exe'
$shell = New-Object -ComObject WScript.Shell
$places = @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('Programs'))
foreach ($place in $places) {
    $lnk = $shell.CreateShortcut((Join-Path $place 'VideoPipeline.lnk'))
    $lnk.TargetPath = $conhost
    $lnk.Arguments = '"' + $exe + '"'
    $lnk.WorkingDirectory = [Environment]::GetFolderPath('MyVideos')
    $lnk.IconLocation = "$exe,0"
    $lnk.Description = 'VideoPipeline - kodowanie wideo na GPU'
    $lnk.Save()
    Write-Host "Utworzono skrot: $($lnk.FullName)"
}
Write-Host 'Gotowe. Skrot mozna tez przypiac do paska zadan (prawy przycisk -> Przypnij do paska zadan).'
