[CmdletBinding()]
param(
    [string[]]$Paths = @(
        'manage.ps1',
        'tools/windows-process-smoke.ps1',
        'tools/with_msvc.ps1',
        '.github/scripts/check-powershell.ps1'
    )
)

$ErrorActionPreference = 'Stop'
$parseFailed = $false

foreach ($path in $Paths) {
    $resolved = Resolve-Path -LiteralPath $path
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $resolved.Path,
        [ref]$tokens,
        [ref]$errors
    ) | Out-Null

    if ($errors.Count -eq 0) {
        Write-Host "PowerShell syntax OK: $path"
        continue
    }

    $parseFailed = $true
    foreach ($parseError in $errors) {
        Write-Error -Message "${path}:$($parseError.Extent.StartLineNumber): $($parseError.Message)" -ErrorAction Continue
    }
}

if ($parseFailed) {
    exit 1
}
