param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Args)
$vcvars = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsarm64.bat"
$envOutput = cmd /c "`"$vcvars`" >nul && set" 2>$null
foreach ($line in $envOutput) {
  if ($line -match '^([^=]+)=(.*)$') {
    Set-Item -Path "env:$($matches[1])" -Value $matches[2] -ErrorAction SilentlyContinue
  }
}
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
& $Args[0] @($Args[1..($Args.Length-1)])
exit $LASTEXITCODE
