; Built by scripts/package-release.ps1 from a validated portable payload.
; PAYLOAD_INCLUDE contains explicit InstallPayload and RemovePayload macros.
Unicode true
RequestExecutionLevel user
ManifestSupportedOS Win10
ManifestDPIAware true

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "Sections.nsh"
!include "WinVer.nsh"
!include "x64.nsh"

!ifndef APP_VERSION
  !error "APP_VERSION is required; use scripts/package-release.ps1."
!endif
!ifndef PRODUCT_VERSION
  !error "PRODUCT_VERSION is required."
!endif
!ifndef OUTPUT_FILE
  !error "OUTPUT_FILE is required."
!endif
!ifndef PAYLOAD_INCLUDE
  !error "PAYLOAD_INCLUDE is required."
!endif
!ifndef ESTIMATED_SIZE
  !error "ESTIMATED_SIZE is required (KiB)."
!endif
!ifndef NSIS_NOTICES
  !error "NSIS_NOTICES must identify the compiler's COPYING file."
!endif
!include "${PAYLOAD_INCLUDE}"

!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\DiskScope"
Name "DiskScope ${APP_VERSION}"
OutFile "${OUTPUT_FILE}"
; An empty default lets .onInit distinguish NSIS's /D override from no override.
; NSIS consumes /D itself; it must be the final command-line argument.
InstallDir ""
AllowRootDirInstall false
SetCompressor /SOLID lzma
SetCompressorDictSize 32
SetDatablockOptimize on
SetOverwrite on
AllowSkipFiles off
ShowInstDetails show
ShowUninstDetails show
BrandingText "DiskScope"
VIProductVersion "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=1033 "ProductName" "DiskScope"
VIAddVersionKey /LANG=1033 "FileDescription" "DiskScope per-user setup"
VIAddVersionKey /LANG=1033 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" ""

Var ExistingInstallDir
Var RegisteredInstall
Var NoShortcuts

!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Install DiskScope ${APP_VERSION}"
!define MUI_WELCOMEPAGE_TEXT "DiskScope will be installed for your Windows account.$\r$\n$\r$\nRequires Windows 10 or later on an Intel or AMD 64-bit PC and graphics support for OpenGL 2.0 or later.$\r$\n$\r$\nClose DiskScope before installing or updating. Your scan history and preferences are preserved."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE ValidateInstallDirectory
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TITLE "DiskScope installation complete"
!define MUI_FINISHPAGE_TEXT "DiskScope has been installed for your Windows account.$\r$\n$\r$\nYour scan history and preferences are preserved."
!insertmacro MUI_PAGE_FINISH
!define MUI_UNCONFIRMPAGE_TEXT_TOP "Remove DiskScope and its installed shortcuts? Scan history, preferences, and other files you added will be kept."
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "English"

; NSIS's GetFullPathName instruction requires an existing path and clears its
; output for a new destination. Use the Windows API for lexical normalization.
; The stack argument/result is the path; preserve all working registers.
!macro DefineNormalizeInstallPath PREFIX
Function ${PREFIX}NormalizeInstallPath
  Exch $0
  Push $1
  Push $2
  ${If} $0 == ""
    Goto normalize_path_failed
  ${EndIf}
  ; Accept fully qualified drive or UNC paths, excluding device namespaces.
  StrCpy $1 $0 4
  ${If} $1 == "\\?\"
  ${OrIf} $1 == "\\.\"
    Goto normalize_path_failed
  ${EndIf}
  StrCpy $1 $0 2
  ${If} $1 != "\\"
    StrCpy $1 $0 1 1
    ${If} $1 != ":"
      Goto normalize_path_failed
    ${EndIf}
    StrCpy $1 $0 1 2
    ${If} $1 != "\"
    ${AndIf} $1 != "/"
      Goto normalize_path_failed
    ${EndIf}
  ${EndIf}
  System::Call 'kernel32::GetFullPathNameW(w r0, i ${NSIS_MAX_STRLEN}, w .r1, p 0) i .r2'
  ${If} $2 == 0
  ${OrIf} $2 >= ${NSIS_MAX_STRLEN}
    Goto normalize_path_failed
  ${EndIf}
  StrCpy $0 $1
normalize_trim_separator:
  StrCpy $1 $0 1 -1
  ${If} $1 == "\"
    StrCpy $0 $0 -1
    Goto normalize_trim_separator
  ${EndIf}
  ; GetRoot only parses the string. Reject drive/share roots and incomplete UNC
  ; names in both interactive and silent modes, before any directory is created.
  ${GetRoot} $0 $1
  ${If} $1 == ""
  ${OrIf} $0 == $1
    Goto normalize_path_failed
  ${EndIf}
  ClearErrors
  Goto normalize_path_done
normalize_path_failed:
  StrCpy $0 ""
  SetErrors
normalize_path_done:
  Pop $2
  Pop $1
  Exch $0
FunctionEnd
!macroend
!insertmacro DefineNormalizeInstallPath ""
!insertmacro DefineNormalizeInstallPath "un."

!macro NormalizeInstallPathOrAbort PREFIX PATH
  Push ${PATH}
  Call ${PREFIX}NormalizeInstallPath
  Pop ${PATH}
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONSTOP "Choose a valid, fully qualified program folder, such as C:\Users\YourName\AppData\Local\Programs\DiskScope. A drive or network-share root cannot be used." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
!macroend

; Opening an existing image for writing fails while Windows maps it for execution.
; This also rejects a read-only/unwritable image before changing the installation.
!macro CheckApplicationWritable PREFIX
Function ${PREFIX}CheckApplicationWritable
  IfFileExists "$INSTDIR\DiskScope.exe" 0 app_write_check_done
  System::Call 'kernel32::CreateFileW(w "$INSTDIR\DiskScope.exe", i 0x40000000, i 7, p 0, i 3, i 0x80, p 0) p .r0'
  ${If} $0 == -1
    MessageBox MB_OK|MB_ICONSTOP "DiskScope is running or its program file cannot be changed. Close DiskScope and check write access to the installation folder, then try again." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  System::Call 'kernel32::CloseHandle(p r0)'
app_write_check_done:
FunctionEnd
!macroend
!insertmacro CheckApplicationWritable ""
!insertmacro CheckApplicationWritable "un."

Function ValidateInstallDirectory
  !insertmacro NormalizeInstallPathOrAbort "" $INSTDIR
  ${If} $ExistingInstallDir != ""
    ${If} $INSTDIR != $ExistingInstallDir
      MessageBox MB_OK|MB_ICONSTOP "DiskScope is already installed in:$\r$\n$ExistingInstallDir$\r$\n$\r$\nUpdate it in that folder. To move it, uninstall the existing copy first; history and preferences will be kept." /SD IDOK
      SetErrorLevel 2
      Abort
    ${EndIf}
  ${EndIf}
  ; Keep the program payload out of the two folders owned by application data.
  ${If} $INSTDIR == "$LOCALAPPDATA\DiskScope"
  ${OrIf} $INSTDIR == "$APPDATA\DiskScope"
    MessageBox MB_OK|MB_ICONSTOP "Choose a program folder separate from DiskScope's history and preferences folders." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
FunctionEnd

Section "DiskScope application (required)" SEC_APPLICATION
  SectionIn RO
  Call ValidateInstallDirectory
  Call CheckApplicationWritable
  ClearErrors
  !insertmacro InstallPayload
  IfErrors payload_copy_failed
  SetOutPath "$INSTDIR"
  IfErrors payload_copy_failed
  File /oname=INSTALLER_NOTICES.txt "${NSIS_NOTICES}"
  IfErrors payload_copy_failed
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  IfErrors payload_copy_failed

  ; Publish uninstall metadata only after every payload file and the uninstaller
  ; have been written successfully. A failed update retains its existing entry.
  ClearErrors
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "DiskScope"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" '"$INSTDIR\DiskScope.exe",0'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" ${ESTIMATED_SIZE}
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  IfErrors registration_failed
  SetErrorLevel 0
  Goto application_done

payload_copy_failed:
  MessageBox MB_OK|MB_ICONSTOP "DiskScope could not copy all required files. Close DiskScope, check free space and write access, then run setup again. The installation did not complete." /SD IDOK
  SetErrorLevel 2
  Abort
registration_failed:
  MessageBox MB_OK|MB_ICONSTOP "DiskScope files were copied, but Windows uninstall registration failed. Run setup again to repair the installation." /SD IDOK
  SetErrorLevel 2
  Abort
application_done:
SectionEnd

Section "Start menu shortcut" SEC_STARTMENU
  ClearErrors
  CreateDirectory "$SMPROGRAMS\DiskScope"
  CreateShortcut "$SMPROGRAMS\DiskScope\DiskScope.lnk" "$INSTDIR\DiskScope.exe" "" "$INSTDIR\DiskScope.exe" 0
  IfErrors startmenu_failed
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "StartMenuShortcut" 1
  IfErrors startmenu_failed
  Goto startmenu_done
startmenu_failed:
  MessageBox MB_OK|MB_ICONSTOP "DiskScope was installed, but its Start menu shortcut could not be created or recorded. Run setup again to repair the shortcut." /SD IDOK
  SetErrorLevel 2
  Abort
startmenu_done:
SectionEnd

Section /o "Desktop shortcut" SEC_DESKTOP
  ClearErrors
  CreateShortcut "$DESKTOP\DiskScope.lnk" "$INSTDIR\DiskScope.exe" "" "$INSTDIR\DiskScope.exe" 0
  IfErrors desktop_failed
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "DesktopShortcut" 1
  IfErrors desktop_failed
  Goto desktop_done
desktop_failed:
  MessageBox MB_OK|MB_ICONSTOP "DiskScope was installed, but its desktop shortcut could not be created or recorded. Run setup again to repair the shortcut." /SD IDOK
  SetErrorLevel 2
  Abort
desktop_done:
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_APPLICATION} "DiskScope and its documentation. Requires Windows 10 or later on native x64 hardware."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_STARTMENU} "Add a DiskScope shortcut to your account's Start menu."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} "Add a DiskScope shortcut to your account's desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function .onInit
  SetShellVarContext current
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "DiskScope requires Windows 10 or later." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  ${IfNot} ${IsNativeAMD64}
    MessageBox MB_OK|MB_ICONSTOP "This DiskScope package requires an Intel or AMD 64-bit PC. Windows on ARM and 32-bit Windows are not supported by this package." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  SetRegView 64
  ReadRegStr $ExistingInstallDir HKCU "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $ExistingInstallDir != ""
    !insertmacro NormalizeInstallPathOrAbort "" $ExistingInstallDir
  ${EndIf}
  ${If} $INSTDIR == ""
    ${If} $ExistingInstallDir != ""
      StrCpy $INSTDIR $ExistingInstallDir
    ${Else}
      StrCpy $INSTDIR "$LOCALAPPDATA\Programs\DiskScope"
    ${EndIf}
  ${EndIf}
  Call ValidateInstallDirectory
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/NoShortcuts" $1
  ${IfNot} ${Errors}
    StrCpy $NoShortcuts 1
    SectionSetFlags ${SEC_STARTMENU} 0
    SectionSetFlags ${SEC_DESKTOP} 0
  ${EndIf}
FunctionEnd

; Keep /NoShortcuts authoritative even if an interactive user opens Components.
Function .onSelChange
  ${If} $NoShortcuts == 1
    SectionSetFlags ${SEC_STARTMENU} 0
    SectionSetFlags ${SEC_DESKTOP} 0
  ${EndIf}
FunctionEnd

Function un.onInit
  SetShellVarContext current
  SetRegView 64
  StrCpy $RegisteredInstall 0
  ReadRegStr $ExistingInstallDir HKCU "${UNINSTALL_KEY}" "InstallLocation"
  !insertmacro NormalizeInstallPathOrAbort "un." $INSTDIR
  ${If} $ExistingInstallDir != ""
    !insertmacro NormalizeInstallPathOrAbort "un." $ExistingInstallDir
    ${If} $ExistingInstallDir != $INSTDIR
      MessageBox MB_OK|MB_ICONSTOP "This uninstaller is not in DiskScope's registered installation folder. Use the uninstaller in:$\r$\n$ExistingInstallDir" /SD IDOK
      SetErrorLevel 2
      Abort
    ${EndIf}
    StrCpy $RegisteredInstall 1
  ${EndIf}
  Call un.CheckApplicationWritable
FunctionEnd

Section "Uninstall"
  ; Check again after the confirmation page, then delete the executable before
  ; other files so a newly running application cannot cause a partial cleanup.
  Call un.CheckApplicationWritable
  SetOutPath "$TEMP"
  Delete "$INSTDIR\DiskScope.exe"
  IfFileExists "$INSTDIR\DiskScope.exe" un.PayloadRemoveFailed
  ; Generated Delete commands check IfFileExists and jump to the failure label.
  ; Generated RMDir commands remove empty directories only; foreign files stay.
  !insertmacro RemovePayload
  Delete "$INSTDIR\INSTALLER_NOTICES.txt"
  IfFileExists "$INSTDIR\INSTALLER_NOTICES.txt" un.PayloadRemoveFailed

  ${If} $RegisteredInstall == 1
    ReadRegDWORD $0 HKCU "${UNINSTALL_KEY}" "StartMenuShortcut"
    ${If} $0 == 1
      Delete "$SMPROGRAMS\DiskScope\DiskScope.lnk"
      IfFileExists "$SMPROGRAMS\DiskScope\DiskScope.lnk" un.PayloadRemoveFailed
      RMDir "$SMPROGRAMS\DiskScope"
    ${EndIf}
    ReadRegDWORD $0 HKCU "${UNINSTALL_KEY}" "DesktopShortcut"
    ${If} $0 == 1
      Delete "$DESKTOP\DiskScope.lnk"
      IfFileExists "$DESKTOP\DiskScope.lnk" un.PayloadRemoveFailed
    ${EndIf}
    ClearErrors
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    IfErrors un.PayloadRemoveFailed
  ${EndIf}
  Delete "$INSTDIR\Uninstall.exe"
  IfFileExists "$INSTDIR\Uninstall.exe" un.PayloadRemoveFailed
  RMDir "$INSTDIR"
  ; A nonempty program directory is intentionally retained, not an uninstall error.
  ClearErrors
  SetErrorLevel 0
  Goto un.done

un.PayloadRemoveFailed:
  MessageBox MB_OK|MB_ICONSTOP "Some DiskScope files or registration could not be removed. Close DiskScope, check write access, and run this uninstaller again. Other files, scan history, and preferences have been kept." /SD IDOK
  SetErrorLevel 2
  Abort
un.done:
SectionEnd
