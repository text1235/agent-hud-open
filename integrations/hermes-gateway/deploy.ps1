param([string]$ProjectRoot='Z:\Agent HUD',[string]$TargetRoot='Z:\Hermes\project-gateway')
$ErrorActionPreference='Stop'
$sourceRoot=$PSScriptRoot
$oldRecord=Get-Content -LiteralPath (Join-Path $TargetRoot 'state\process.json') -Raw | ConvertFrom-Json
$python=$oldRecord.path
$oldProcess=Get-Process -Id $oldRecord.pid -ErrorAction Stop
$identity=Get-CimInstance Win32_Process -Filter ('ProcessId='+$oldRecord.pid)
if($identity.ExecutablePath -ne $python -or $identity.CommandLine -notlike ('*'+(Join-Path $TargetRoot 'gateway.py')+'*serve*')){throw 'Gateway process identity mismatch'}
$rootPath=(Resolve-Path -LiteralPath $ProjectRoot).Path
$configFile=Join-Path $TargetRoot 'state\native-desktop.json'
if(Test-Path -LiteralPath $configFile){throw 'Native config already exists; preserve and merge it explicitly'}
$node=(Get-Command node -ErrorAction Stop).Source
foreach($name in @('gateway.py','observations.py','native_desktop.py','desktop_rpc.cjs')){
    if(-not(Test-Path -LiteralPath (Join-Path $sourceRoot $name))){throw "Missing source: $name"}
}
foreach($name in @('gateway.py','observations.py')){
    $baseline=(& git -C (Join-Path $sourceRoot '..\..') show ('a145ec0:integrations/hermes-gateway/'+$name)) -join "`n"
    if($LASTEXITCODE){throw 'Git baseline unavailable'}
    $current=[IO.File]::ReadAllText((Join-Path $TargetRoot $name)).Replace("`r`n","`n").TrimEnd("`r","`n")
    if($current -ne $baseline.TrimEnd("`r","`n")){throw "Existing $name changed since baseline; merge required"}
}
$backup=Join-Path $TargetRoot ('backups\native-'+(Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $backup -Force | Out-Null
foreach($name in @('gateway.py','observations.py','native_desktop.py','desktop_rpc.cjs')){
    $current=Join-Path $TargetRoot $name
    if(Test-Path -LiteralPath $current){Copy-Item -LiteralPath $current -Destination (Join-Path $backup $name)}
}
& $python -X utf8 -c 'import sqlite3,sys; from pathlib import Path; s=sqlite3.connect(Path(sys.argv[1]).as_uri()+"?mode=ro",uri=True); d=sqlite3.connect(sys.argv[2]); s.backup(d); d.close(); s.close()' (Join-Path $TargetRoot 'state\tasks.db') (Join-Path $backup 'tasks.db')
if($LASTEXITCODE){throw 'Database backup failed'}
$token=([IO.File]::ReadAllText((Join-Path $TargetRoot 'state\gateway.token'))).Trim()
$headers=@{Authorization='Bearer '+$token}
$oldExited=$false
$configCreated=$false
$newProcess=$null
function Start-GatewayService([string]$logPrefix){
    Start-Process -FilePath $python -ArgumentList @((Join-Path $TargetRoot 'gateway.py'),'serve') -WorkingDirectory $TargetRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $backup ($logPrefix+'.stdout.log')) -RedirectStandardError (Join-Path $backup ($logPrefix+'.stderr.log'))
}
try {
    $shutdown=Invoke-RestMethod -Uri 'http://127.0.0.1:18493' -Method Post -ContentType 'application/json' -Headers $headers -Body '{"name":"_shutdown","arguments":{}}' -NoProxy -TimeoutSec 10
    if($shutdown.error){throw $shutdown.error}
    if(-not $shutdown.result.stopping){throw 'Gateway did not accept normal shutdown'}
    if(-not $oldProcess.WaitForExit(30000)){throw 'Gateway has not exited; no files replaced and no force termination performed'}
    $oldExited=$true
    foreach($name in @('gateway.py','observations.py','native_desktop.py','desktop_rpc.cjs')){Copy-Item -LiteralPath (Join-Path $sourceRoot $name) -Destination (Join-Path $TargetRoot $name) -Force}
    $config=@{projects=@($rootPath);publication=(Join-Path $env:USERPROFILE '.local\state\hermes\gateway-locks\host-desktop-serve.json');ownership='Z:\Hermes\data\desktop\backend-ownership.json';profile='default';node=$node;model='agnes-3.0-flash';provider='relay'}
    $config | ConvertTo-Json | Set-Content -LiteralPath $configFile -Encoding utf8
    $configCreated=$true
    $newProcess=Start-GatewayService 'new-service'
    $record=[ordered]@{task='User requested existing Project Gateway native binding upgrade';path=$python;pid=$newProcess.Id;startedAt=$newProcess.StartTime.ToString('o');oldPid=$oldRecord.pid;oldExited=$true;backup=$backup;keepRunning=$true;hermesOrWeixinRestarted=$false;projectRoot=$rootPath}
    $record | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $backup 'upgrade.json') -Encoding utf8
    $result=$null
    for($attempt=0;$attempt -lt 12;$attempt++){
        if($newProcess.HasExited){throw 'Updated Gateway exited'}
        try{$result=Invoke-RestMethod -Uri 'http://127.0.0.1:18493' -Method Post -ContentType 'application/json' -Headers $headers -Body (@{name='gateway_ensure_project';arguments=@{workspace=$rootPath}} | ConvertTo-Json) -NoProxy -TimeoutSec 3; if($result.result.id){break}}catch{}
        Start-Sleep -Milliseconds 250
    }
    if(-not $result.result.id){throw 'Updated Gateway is not responding'}
    $record.projectId=$result.result.id
    $record.healthy=$true
    $record | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $backup 'upgrade.json') -Encoding utf8
    $record | ConvertTo-Json
} catch {
    $problem=$_.Exception.Message
    if($oldExited -and (-not $newProcess -or $newProcess.HasExited)){
        foreach($name in @('gateway.py','observations.py')){Copy-Item -LiteralPath (Join-Path $backup $name) -Destination (Join-Path $TargetRoot $name) -Force}
        if($configCreated){Remove-Item -LiteralPath $configFile}
        $restored=Start-GatewayService 'restored-service'
        @{task='Restore existing Project Gateway';path=$python;pid=$restored.Id;startedAt=$restored.StartTime.ToString('o');keepRunning=$true;reason=$problem} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $backup 'rollback.json') -Encoding utf8
    }
    throw ($problem+'; backup: '+$backup+'; no process was force-terminated')
}
