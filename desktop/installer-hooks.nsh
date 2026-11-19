!macro GuardTangledLink
    System::Call 'kernel32::SetEnvironmentVariableW(w "TANGLEDLINK_INSTALL_DIR", w "$INSTDIR") i .r0'
    StrCmp $0 "0" +4
    nsExec::Exec '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -Command "$$ErrorActionPreference = [System.Management.Automation.ActionPreference]::Stop; if (Get-CimInstance Win32_Process | Where-Object ExecutablePath -eq (Join-Path $$env:TANGLEDLINK_INSTALL_DIR TangledLink.exe)) { exit 1 }; exit 0"'
    Pop $0
    StrCmp $0 "0" +5
    IfSilent +2
    MessageBox MB_OK|MB_ICONEXCLAMATION "Close TangledLink Desktop from its system tray menu before continuing."
    SetErrorLevel 1
    Abort
!macroend

!macro NSIS_HOOK_PREINSTALL
    !insertmacro GuardTangledLink
!macroend

!macro NSIS_HOOK_PREUNINSTALL
    !insertmacro GuardTangledLink
!macroend
