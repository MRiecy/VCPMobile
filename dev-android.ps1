# VCP Mobile Android Debug 开发启动脚本
# 在当前终端直接运行 Tauri dev，后台自动检测安装并用正确包名启动

$adb = "C:\Users\32595\AppData\Local\Android\Sdk\platform-tools\adb.exe"
$packageName = "com.vcp.avatar.debug"
$activityName = "com.vcp.avatar.MainActivity"

# 启动后台作业：轮询等待 APK 安装，然后自动用正确包名启动
$launcherJob = Start-Job -ScriptBlock {
    param($adbPath, $pkg, $act)
    
    for ($i = 0; $i -lt 120; $i += 5) {
        Start-Sleep -Seconds 5
        
        $found = & $adbPath shell pm list packages $pkg 2>$null
        if ($found -match $pkg) {
            & $adbPath shell am start -n "$pkg/$act" 2>$null
            Write-Output "=== VCP-Debug launched with correct package name ==="
            break
        }
    }
} -ArgumentList $adb, $packageName, $activityName

# 前台直接运行 Tauri dev（阻塞式，你能看到完整日志）
try {
    pnpm tauri android dev
} finally {
    # Tauri dev 结束后清理后台作业
    Stop-Job $launcherJob -ErrorAction SilentlyContinue
    Remove-Job $launcherJob -ErrorAction SilentlyContinue
}
