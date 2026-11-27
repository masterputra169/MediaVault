; MediaVault 0.5.0 — 100% Pure-Rust Native Windows NSIS Installer (Per-User, Selectable Folder)
!include "MUI2.nsh"

Name "MediaVault"
OutFile "target\release\MediaVault_0.5.0_x64-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\MediaVault"
InstallDirRegKey HKCU "Software\MediaVault" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
Unicode true

!define MUI_ABORTWARNING
!define MUI_ICON "assets\icon.ico"
!define MUI_UNICON "assets\icon.ico"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\MediaVault.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch MediaVault now"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "MediaVault" SecMain
  SetOutPath "$INSTDIR"
  File "/oname=MediaVault.exe" "target\release\mediavault.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\MediaVault"
  CreateShortcut "$SMPROGRAMS\MediaVault\MediaVault.lnk" "$INSTDIR\MediaVault.exe"
  CreateShortcut "$SMPROGRAMS\MediaVault\Uninstall MediaVault.lnk" "$INSTDIR\Uninstall.exe"
  CreateShortcut "$DESKTOP\MediaVault.lnk" "$INSTDIR\MediaVault.exe"

  WriteRegStr HKCU "Software\MediaVault" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MediaVault" "DisplayName" "MediaVault"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MediaVault" "DisplayVersion" "0.5.0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MediaVault" "Publisher" "MediaVault"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MediaVault" "DisplayIcon" "$INSTDIR\MediaVault.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MediaVault" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MediaVault" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MediaVault" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\MediaVault.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\MediaVault\MediaVault.lnk"
  Delete "$SMPROGRAMS\MediaVault\Uninstall MediaVault.lnk"
  RMDir "$SMPROGRAMS\MediaVault"
  Delete "$DESKTOP\MediaVault.lnk"

  DeleteRegKey HKCU "Software\MediaVault"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MediaVault"
SectionEnd
