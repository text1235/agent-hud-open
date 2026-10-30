param(
    [string]$ToolsRoot='Z:\Agent HUD\build-tools',
    [string]$EvidenceRoot='Z:\Agent HUD\测试记录\0.1.2-local-build'
)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$tools=(Resolve-Path -LiteralPath $ToolsRoot).Path
$portable=Join-Path $tools 'portable-msvc\msvc'
$vc=@(Get-ChildItem -LiteralPath (Join-Path $portable 'VC\Tools\MSVC') -Directory)
$sdk=@(Get-ChildItem -LiteralPath (Join-Path $portable 'Windows Kits\10\Include') -Directory)
if($vc.Count -ne 1 -or $sdk.Count -ne 1){throw 'Expected one prepared compiler and SDK'}
$vcRoot=$vc[0].FullName
$sdkRoot=Join-Path $portable 'Windows Kits\10'
$sdkVersion=$sdk[0].Name
$vcBin=Join-Path $vcRoot 'bin\Hostx64\x64'
$env:CARGO_HOME=Join-Path $tools 'cargo'
$env:RUSTUP_HOME=Join-Path $tools 'rustup'
$env:CARGO_NET_GIT_FETCH_WITH_CLI='true'
$env:CARGO_BUILD_JOBS='1'
$env:CARGO_PROFILE_DEV_DEBUG='0'
$env:CARGO_PROFILE_TEST_DEBUG='0'
$env:RUST_TEST_THREADS='1'
$env:TEMP=Join-Path $tools 'tmp'
$env:TMP=$env:TEMP
$env:BUN_EXECUTABLE=Join-Path $tools 'bun\bun-windows-x64\bun.exe'
$env:PATH=(Join-Path $env:CARGO_HOME 'bin')+';'+$vcBin+';'+(Join-Path $sdkRoot "bin\$sdkVersion\x64")+';'+(Split-Path $env:BUN_EXECUTABLE)+';'+$env:PATH
$env:INCLUDE=(Join-Path $vcRoot 'include')+';'+((@('ucrt','shared','um','winrt','cppwinrt') | ForEach-Object {Join-Path $sdkRoot "Include\$sdkVersion\$_"}) -join ';')
$env:LIB=(Join-Path $vcRoot 'lib\x64')+';'+(Join-Path $sdkRoot "Lib\$sdkVersion\ucrt\x64")+';'+(Join-Path $sdkRoot "Lib\$sdkVersion\um\x64")
$env:VCToolsInstallDir=$vcRoot+'\'
$env:VCToolsVersion=$vc[0].Name
$env:WindowsSdkDir=$sdkRoot+'\'
$env:WindowsSDKVersion=$sdkVersion+'\'
$env:VSCMD_ARG_HOST_ARCH='x64'
$env:VSCMD_ARG_TGT_ARCH='x64'
$env:CC=Join-Path $vcBin 'cl.exe'
$env:CXX=$env:CC
$env:AR=Join-Path $vcBin 'lib.exe'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=Join-Path $vcBin 'link.exe'
$cargo=Join-Path $env:CARGO_HOME 'bin\cargo.exe'
$node=(Get-Command node.exe -ErrorAction Stop).Source
$pnpmBin=Split-Path (Get-Command pnpm.cmd -ErrorAction Stop).Source
$pnpm=[IO.Path]::GetFullPath((Join-Path $pnpmBin '..\..\node\node_modules\pnpm\bin\pnpm.mjs'))
foreach($path in @($cargo,$env:CC,$env:AR,$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER,$env:BUN_EXECUTABLE,$pnpm)){
    if(-not(Test-Path -LiteralPath $path)){throw "Missing prepared dependency: $path"}
}
New-Item -ItemType Directory -Path $EvidenceRoot,$env:TEMP,(Join-Path $root 'src\apps\desktop\src-tauri\resources') -Force | Out-Null
function Invoke-Phase([string]$Name,[string]$Exe,[string[]]$Arguments){
    $log=Join-Path $EvidenceRoot $Name
    $quoted=@($Arguments | ForEach-Object {'"'+$_+'"'})
    $process=Start-Process -FilePath $Exe -ArgumentList $quoted -WorkingDirectory $root -WindowStyle Hidden -PassThru -RedirectStandardOutput ($log+'.stdout.log') -RedirectStandardError ($log+'.stderr.log')
    $record=[ordered]@{task='Agent HUD 0.1.2 local build';phase=$Name;path=$Exe;args=$Arguments;pid=$process.Id;startedAt=$process.StartTime.ToString('o')}
    $record | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath ($log+'.process.json') -Encoding utf8
    try{
        $process.WaitForExit()
        $record.exitCode=$process.ExitCode
        if($process.ExitCode -ne 0){Get-Content -LiteralPath ($log+'.stderr.log') -Tail 25;throw "$Name failed"}
        Write-Output "$Name passed"
    }finally{
        if(-not $process.HasExited){[void]$process.CloseMainWindow()}
        $record.exited=$process.HasExited
        $record.residual=-not $process.HasExited
        $record.endedAt=(Get-Date).ToString('o')
        $record | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath ($log+'.process.json') -Encoding utf8
    }
}
Invoke-Phase 'clippy' $cargo @('clippy','--workspace','--tests','--locked','--','-D','warnings')
Invoke-Phase 'native-tests' $cargo @('test','--workspace','--locked','--no-fail-fast')
Invoke-Phase 'installer' $node @($pnpm,'run','build:windows','--','--bundle')
$installers=@(Get-ChildItem -LiteralPath (Join-Path $root 'target\release\bundle\nsis') -Filter '*-setup.exe')
if($installers.Count -ne 1){throw 'Expected one installer'}
$revision=(& git -C $root rev-parse HEAD).Trim()
$changes=& git -C $root status --porcelain --untracked-files=no
if($changes){throw 'Tracked source changed during build; provenance needs review'}
@{revision=$revision;build='local-windows';version='0.1.2';signed=$false;compiler=$vc[0].Name;sdk=$sdkVersion;installer=$installers[0].Name;sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $installers[0].FullName).Hash.ToLower()} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $EvidenceRoot 'provenance.json') -Encoding utf8
