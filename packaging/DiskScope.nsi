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
!ifndef SETUP_ICON
  !error "SETUP_ICON is required; use scripts/package-release.ps1."
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
!define MUI_ICON "${SETUP_ICON}"
!define MUI_UNICON "${SETUP_ICON}"
!define MUI_WELCOMEPAGE_TITLE "Install DiskScope ${APP_VERSION}"
!define MUI_WELCOMEPAGE_TEXT "DiskScope will be installed for your Windows account.$\r$\n$\r$\nRequires Windows 10 or later on an Intel or AMD 64-bit PC and a graphics driver with Direct3D 12 or OpenGL 2.0 support.$\r$\n$\r$\nClose DiskScope before installing or updating. Your scan history and preferences are preserved."
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

; An existing installation lists its own payload in SHA256SUMS.txt: a hash, two
; spaces and a forward-slash relative path. The manifest does not list itself.
; Those files are removed before this version writes its payload, so an update
; never leaves files of the version it replaces behind, such as the notices of a
; dependency that is no longer shipped. Only manifest-listed relative paths
; inside $INSTDIR are touched: files the user added are not listed and stay.
; Nothing is removed unless the manifest lists the application itself and every
; listed file can be opened for deletion or carries only the read-only, hidden
; or system attributes, so an update that cannot proceed leaves the previous
; installation as it was.
Var PreviousPayloadRemoved

; $1 holds a manifest line. Leaves in $2 the relative path it names with
; backslash separators, or an empty string when the line is malformed or the
; path could leave $INSTDIR.
Function PreviousPayloadLinePath
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7
  Push $R0
  StrCpy $2 ""
  StrLen $3 $1
  ${If} $3 < 67
    Goto line_path_done
  ${EndIf}
  StrCpy $4 $1 1 64
  ${If} $4 != " "
    Goto line_path_done
  ${EndIf}
  StrCpy $4 $1 1 65
  ${If} $4 != " "
    Goto line_path_done
  ${EndIf}
  StrCpy $2 $1 "" 66
line_path_trim:
  StrCpy $4 $2 1 -1
  ${If} $4 == "$\r"
  ${OrIf} $4 == "$\n"
    StrCpy $2 $2 -1
    Goto line_path_trim
  ${EndIf}
  StrLen $3 $2
  ${If} $3 == 0
    Goto line_path_done
  ${EndIf}
  ; A safe path has no drive, root, wildcard, alternate data stream or parent
  ; reference, so with either separator it can only name something inside
  ; $INSTDIR. A run of dots at the end is "." or "..": both are rejected. Each
  ; accepted forward slash becomes a backslash, so every later file operation
  ; sees one form.
  StrCpy $4 0
  StrCpy $5 ""
  StrCpy $6 0
  StrCpy $R0 ""
line_path_next:
  ; IntCmp jumps on equal, then less, then more.
  IntCmp $4 $3 line_path_checked line_path_step line_path_checked
line_path_step:
  StrCpy $7 $2 1 $4
  ${If} $7 == "\"
  ${OrIf} $7 == "/"
    ${If} $4 == 0
    ${OrIf} $5 == "\"
    ${OrIf} $6 == 2
      Goto line_path_reject
    ${EndIf}
    StrCpy $6 0
    StrCpy $7 "\"
  ${ElseIf} $7 == ":"
  ${OrIf} $7 == "*"
  ${OrIf} $7 == "?"
    Goto line_path_reject
  ${Else}
    ${If} $7 == "."
      IntOp $6 $6 + 1
    ${Else}
      StrCpy $6 0
    ${EndIf}
  ${EndIf}
  StrCpy $5 $7
  StrCpy $R0 "$R0$7"
  IntOp $4 $4 + 1
  Goto line_path_next
line_path_checked:
  ${If} $6 != 0
    Goto line_path_reject
  ${EndIf}
  StrCpy $2 $R0
  Goto line_path_done
line_path_reject:
  StrCpy $2 ""
line_path_done:
  Pop $R0
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
FunctionEnd

; Two passes over the existing installation's manifest. The first only checks
; that every listed file can be opened for deletion; the second removes them.
; $9 selects the pass and $8 records that the manifest is DiskScope's own.
Function RemovePreviousPayload
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7
  Push $8
  Push $9
  StrCpy $PreviousPayloadRemoved 0
  ClearErrors
  IfFileExists "$INSTDIR\SHA256SUMS.txt" 0 previous_done
  StrCpy $9 0
previous_pass:
  ClearErrors
  FileOpen $0 "$INSTDIR\SHA256SUMS.txt" r
  IfErrors previous_done
  StrCpy $8 0
previous_line:
  ClearErrors
  FileRead $0 $1
  IfErrors previous_closed
  Call PreviousPayloadLinePath
  ${If} $2 == ""
    Goto previous_line
  ${EndIf}
  ${If} $2 == "SHA256SUMS.txt"
    Goto previous_line
  ${EndIf}
  ${If} $2 == "DiskScope.exe"
    StrCpy $8 1
  ${EndIf}
  IfFileExists "$INSTDIR\$2" 0 previous_line
  ${If} $9 == 0
    StrCpy $5 0
previous_probe:
    StrCpy $7 -1
    System::Call 'kernel32::CreateFileW(w "$INSTDIR\$2", i 0x00010000, i 7, p 0, i 3, i 0x80, p 0) p .r7'
    ${If} $7 != -1
      System::Call 'kernel32::CloseHandle(p r7)'
      Goto previous_line
    ${EndIf}
    ; The read-only, hidden and system attributes alone refuse that open; the
    ; removal pass clears them, as overwriting such a file always has. Anything
    ; else is a file in use or no longer readable. A short-lived scanner or
    ; indexer hold gets a few retries; after that nothing at all may be removed.
    System::Call 'kernel32::GetFileAttributesW(w "$INSTDIR\$2") i .r6'
    IntOp $4 $6 & 7
    ${If} $6 >= 0
    ${AndIf} $4 != 0
      Goto previous_line
    ${EndIf}
    IntOp $5 $5 + 1
    ${If} $5 < 5
      Sleep 200
      Goto previous_probe
    ${EndIf}
    FileClose $0
    MessageBox MB_OK|MB_ICONSTOP "A file in the existing DiskScope installation is in use or can no longer be opened. Close DiskScope and any program that has its files open, then try again. Nothing has been changed." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  ClearErrors
  Delete "$INSTDIR\$2"
  IfFileExists "$INSTDIR\$2" 0 previous_removed
  SetFileAttributes "$INSTDIR\$2" NORMAL
  ClearErrors
  Delete "$INSTDIR\$2"
  IfFileExists "$INSTDIR\$2" 0 previous_removed
  DetailPrint "Left in place because it could not be removed: $2"
  Goto previous_line
previous_removed:
  IntOp $PreviousPayloadRemoved $PreviousPayloadRemoved + 1
  ; Remove the file's own folder too when that left it empty.
  StrLen $3 $2
previous_parent:
  IntOp $3 $3 - 1
  ; IntCmp jumps on equal, then less, then more: stop at or before the first character.
  IntCmp $3 0 previous_line previous_line previous_parent_step
previous_parent_step:
  StrCpy $4 $2 1 $3
  ${If} $4 == "\"
  ${OrIf} $4 == "/"
    StrCpy $5 $2 $3
    RMDir "$INSTDIR\$5"
    Goto previous_line
  ${EndIf}
  Goto previous_parent
previous_closed:
  FileClose $0
  ; A manifest that does not list the application is not DiskScope's own.
  ${If} $8 == 0
    Goto previous_done
  ${EndIf}
  ${If} $9 == 0
    StrCpy $9 1
    Goto previous_pass
  ${EndIf}
  ${If} $PreviousPayloadRemoved > 0
    DetailPrint "Removed $PreviousPayloadRemoved file(s) of the previous DiskScope version."
  ${EndIf}
previous_done:
  ClearErrors
  Pop $9
  Pop $8
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

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
  Call RemovePreviousPayload
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
