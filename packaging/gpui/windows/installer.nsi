Unicode true
!include "MUI2.nsh"
Name "Zest"
OutFile "${Output}"
InstallDir "$LOCALAPPDATA\Zest"
InstallDirRegKey HKCU "Software\zest\Zest" ""
RequestExecutionLevel user
SetCompressor /SOLID lzma
VIProductVersion "${NumericVersion}.0"
VIAddVersionKey /LANG=1033 "ProductName" "Zest"
VIAddVersionKey /LANG=1033 "FileDescription" "Zest installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${Version}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Zest contributors"
!define MUI_ICON "${Art}\icons\icon.ico"
!define MUI_UNICON "${Art}\icons\icon.ico"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP "${Art}\installer\nsis-header.bmp"
!define MUI_WELCOMEFINISHPAGE_BITMAP "${Art}\installer\nsis-sidebar.bmp"
!define UninstallKey "Software\Microsoft\Windows\CurrentVersion\Uninstall\Zest"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${License}"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Zest"
  SetRegView 64
  SetOutPath "$INSTDIR"
  File /oname=zest-desktop.exe "${Binary}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\Zest.lnk" "$INSTDIR\zest-desktop.exe" "--launcher"
  WriteRegStr HKCU "Software\zest\Zest" "" "$INSTDIR"
  WriteRegStr HKCU "${UninstallKey}" "DisplayName" "Zest"
  WriteRegStr HKCU "${UninstallKey}" "Publisher" "zest"
  WriteRegStr HKCU "${UninstallKey}" "DisplayVersion" "${Version}"
  WriteRegStr HKCU "${UninstallKey}" "DisplayIcon" "$INSTDIR\zest-desktop.exe"
  WriteRegStr HKCU "${UninstallKey}" "MainBinaryName" "zest-desktop.exe"
  WriteRegStr HKCU "${UninstallKey}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UninstallKey}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UninstallKey}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UninstallKey}" "NoModify" 1
  WriteRegDWORD HKCU "${UninstallKey}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetRegView 64
  Delete "$SMPROGRAMS\Zest.lnk"
  Delete "$INSTDIR\zest-desktop.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UninstallKey}"
  DeleteRegKey HKCU "Software\zest\Zest"
SectionEnd
