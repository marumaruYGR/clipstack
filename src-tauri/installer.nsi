; SPDX-License-Identifier: Apache-2.0 OR MIT
; Uses the Tauri 2 NSIS bundler variables and standard WebView2 bootstrapper.
Unicode true
ManifestDPIAware true
ManifestDPIAwareness PerMonitorV2
RequestExecutionLevel admin
SetCompressor /SOLID "{{compression}}"

{{#if signed_plugins_path}}
!addplugindir "{{signed_plugins_path}}"
{{/if}}
!addplugindir "{{additional_plugins_path}}"
!include "MUI2.nsh"
!include "nsDialogs.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "Win\RestartManager.nsh"
!include "utils.nsh"

!define PRODUCTNAME "{{product_name}}"
!define MANUFACTURER "{{manufacturer}}"
!searchreplace VERSION "{{version}}" "+" "fix"
!define MAINBINARYNAME "{{main_binary_name}}"
!define BUNDLEID "{{bundle_id}}"
!define ARCH "{{arch}}"
!define INSTALLMODE "{{install_mode}}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define INSTALLKEY "Software\${BUNDLEID}\Installation"
!define PREFKEY "Software\${BUNDLEID}"
!define RUNKEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define RUN_ALL "Clipstack.Win64.AllUsers"
!define WEBVIEW2APPGUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

Name "${PRODUCTNAME}"
OutFile "{{out_file}}"
InstallDir "$PROGRAMFILES64\${PRODUCTNAME}"
VIProductVersion "{{version_with_build}}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

!if "{{uninstaller_sign_cmd}}" != ""
  !uninstfinalize '{{uninstaller_sign_cmd}}'
!endif
!if "{{installer_icon}}" != ""
  !define MUI_ICON "{{installer_icon}}"
!endif
!define MUI_WELCOMEFINISHPAGE_BITMAP "{{sidebar_image}}"
!define MUI_ABORTWARNING

Var ClipScope
Var ClipStartup
Var ClipStartMenu
Var ClipDesktop
Var ClipLaunch
Var ScopeButton
Var StartupButton
Var StartMenuButton
Var DesktopButton
Var LaunchButton
Var SessionSid
Var HelperToken
Var HelperResult
Var PassiveMode

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipPassive
!insertmacro MUI_PAGE_WELCOME
Page custom ScopePage ScopeLeave
Page custom OptionsPage OptionsLeave
!insertmacro MUI_PAGE_INSTFILES
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipPassive
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

{{#each languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}
{{#each language_files}}
!include "{{this}}"
{{/each}}

LangString scopeTitle ${LANG_SIMPCHINESE} "选择安装范围"
LangString scopeSubtitle ${LANG_SIMPCHINESE} "选择哪些 Windows 账户可以从其系统入口使用剪栈。"
LangString scopeAll ${LANG_SIMPCHINESE} "安装给所有人"
LangString scopeCurrent ${LANG_SIMPCHINESE} "安装给本账户"
LangString optionsTitle ${LANG_SIMPCHINESE} "安装选项"
LangString optionsSubtitle ${LANG_SIMPCHINESE} "选择安装完成后的行为和快捷方式。"
LangString optionStartup ${LANG_SIMPCHINESE} "登录 Windows 时自动启动"
LangString optionStartMenu ${LANG_SIMPCHINESE} "在开始菜单创建快捷方式"
LangString optionDesktop ${LANG_SIMPCHINESE} "在桌面创建快捷方式"
LangString optionLaunch ${LANG_SIMPCHINESE} "安装完成后立即启动剪栈"
LangString accountUnavailable ${LANG_SIMPCHINESE} "找不到当前登录的 Windows 桌面账户。请登录桌面后重试，或选择安装给所有人。"
LangString ownerMismatch ${LANG_SIMPCHINESE} "此目录已用于另一 Windows 账户的剪栈安装。请登录原账户并卸载后重试。"
LangString helperFailed ${LANG_SIMPCHINESE} "无法为当前 Windows 账户配置快捷方式或启动设置。请在该账户的桌面重新运行安装程序。"
LangString exitRunningApp ${LANG_SIMPCHINESE} "剪栈仍在运行。请在托盘菜单中选择“退出”，然后点击“重试”。"
LangString scopeTitle ${LANG_ENGLISH} "Installation scope"
LangString scopeSubtitle ${LANG_ENGLISH} "Choose which Windows accounts receive Clipstack's shortcuts and startup entry."
LangString scopeAll ${LANG_ENGLISH} "Install for everyone"
LangString scopeCurrent ${LANG_ENGLISH} "Install for this account"
LangString optionsTitle ${LANG_ENGLISH} "Installation options"
LangString optionsSubtitle ${LANG_ENGLISH} "Choose startup and shortcut options."
LangString optionStartup ${LANG_ENGLISH} "Start automatically when signing in to Windows"
LangString optionStartMenu ${LANG_ENGLISH} "Create a Start menu shortcut"
LangString optionDesktop ${LANG_ENGLISH} "Create a desktop shortcut"
LangString optionLaunch ${LANG_ENGLISH} "Launch Clipstack after installation"
LangString accountUnavailable ${LANG_ENGLISH} "The current Windows desktop account is unavailable. Sign in to the desktop or choose to install for everyone."
LangString ownerMismatch ${LANG_ENGLISH} "The installation directory is already used by another Windows account. Sign in as the original account and uninstall first."
LangString helperFailed ${LANG_ENGLISH} "Could not configure the current account's shortcuts or startup settings. Run the installer again from that account's desktop."
LangString exitRunningApp ${LANG_ENGLISH} "Clipstack is still running. Choose Exit from its tray menu, then click Retry."

; Resolve the account behind the desktop shell, which can differ from the
; administrator who entered credentials at the UAC prompt.
!macro DesktopSid
  StrCpy $SessionSid ""
  System::Call 'user32::GetShellWindow() p .r0'
  ${If} $0 P<> 0
    System::Call 'user32::GetWindowThreadProcessId(p r0, *i .r1)'
    System::Call 'kernel32::OpenProcess(i 0x1000, i 0, i r1) p .r0'
    ${If} $0 P<> 0
      System::Call 'advapi32::OpenProcessToken(p r0, i 8, *p .r2) i .r3'
      System::Call 'kernel32::CloseHandle(p r0)'
      ${If} $3 <> 0
        System::Alloc 256
        Pop $4
        ${If} $4 P<> 0
          System::Call 'advapi32::GetTokenInformation(p r2, i 1, p r4, i 256, *i .r5) i .r3'
          ${If} $3 <> 0
            System::Call '*$4(p .r5)'
            System::Call 'advapi32::ConvertSidToStringSidW(p r5, *p .r6) i .r3'
            ${If} $3 <> 0
              System::Call '*$6(&w${NSIS_MAX_STRLEN} .r7)'
              StrCpy $SessionSid $7
              System::Call 'kernel32::LocalFree(p r6)'
            ${EndIf}
          ${EndIf}
          System::Free $4
        ${EndIf}
        System::Call 'kernel32::CloseHandle(p r2)'
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

!macro RunUserHelper arguments
  StrCpy $HelperResult ""
  System::Call 'ole32::CoCreateGuid(g .r0) i .r1'
  ${If} $1 == 0
    StrCpy $HelperToken $0 36 1
    nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" '${arguments} --installer-result $HelperToken --installer-account $SessionSid'
    Pop $0
    ${If} $0 == 0
      StrCpy $8 0
      ${Do}
        ReadRegStr $HelperResult HKU "$SessionSid\${PREFKEY}" "InstallerResult"
        ${If} $HelperResult == "$HelperToken:ok"
        ${OrIf} $HelperResult == "$HelperToken:error"
          ${ExitDo}
        ${EndIf}
        Sleep 100
        IntOp $8 $8 + 1
      ${LoopWhile} $8 < 150
    ${EndIf}
  ${EndIf}
!macroend

Function SkipPassive
  ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
FunctionEnd

Function .onInit
  !insertmacro SetContext
  SetShellVarContext all
  StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
  StrCpy $ClipScope "AllUsers"
  StrCpy $ClipStartup ${BST_CHECKED}
  StrCpy $ClipStartMenu ${BST_CHECKED}
  StrCpy $ClipDesktop ${BST_UNCHECKED}
  StrCpy $ClipLaunch ${BST_CHECKED}
  ${GetOptions} $CMDLINE "/P" $0
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}
  ${If} ${Silent}
  ${OrIf} $PassiveMode = 1
    StrCpy $ClipLaunch ${BST_UNCHECKED}
    ${GetOptions} $CMDLINE "/CurrentUser" $0
    ${IfNot} ${Errors}
      StrCpy $ClipScope "CurrentUser"
    ${EndIf}
  ${EndIf}
  !insertmacro DesktopSid
FunctionEnd

Function ScopePage
  Call SkipPassive
  !insertmacro MUI_HEADER_TEXT "$(scopeTitle)" "$(scopeSubtitle)"
  nsDialogs::Create 1018
  Pop $0
  ${IfThen} $0 = error ${|} Abort ${|}
  ${NSD_CreateLabel} 0 4u 100% 20u "$INSTDIR"
  Pop $0
  ${NSD_CreateRadioButton} 0 37u 100% 14u "$(scopeAll)"
  Pop $ScopeButton
  ${NSD_CreateRadioButton} 0 60u 100% 14u "$(scopeCurrent)"
  Pop $0
  ${If} $ClipScope == "AllUsers"
    ${NSD_Check} $ScopeButton
  ${Else}
    ${NSD_Check} $0
  ${EndIf}
  nsDialogs::Show
FunctionEnd

Function ScopeLeave
  ${NSD_GetState} $ScopeButton $0
  ${If} $0 = ${BST_CHECKED}
    StrCpy $ClipScope "AllUsers"
  ${Else}
    StrCpy $ClipScope "CurrentUser"
  ${EndIf}
  Call CheckScope
FunctionEnd

Function CheckScope
  ${If} $ClipScope == "CurrentUser"
  ${AndIf} $SessionSid == ""
    MessageBox MB_ICONEXCLAMATION "$(accountUnavailable)"
    Abort
  ${EndIf}
  ReadRegStr $0 HKLM "${INSTALLKEY}" "OwnerSid"
  ${If} $0 != ""
  ${AndIf} $0 != $SessionSid
    MessageBox MB_ICONEXCLAMATION "$(ownerMismatch)"
    Abort
  ${EndIf}
FunctionEnd

Function OptionsPage
  Call SkipPassive
  !insertmacro MUI_HEADER_TEXT "$(optionsTitle)" "$(optionsSubtitle)"
  nsDialogs::Create 1018
  Pop $0
  ${IfThen} $0 = error ${|} Abort ${|}
  ${NSD_CreateCheckbox} 0 14u 100% 13u "$(optionStartup)"
  Pop $StartupButton
  ${NSD_SetState} $StartupButton $ClipStartup
  ${NSD_CreateCheckbox} 0 36u 100% 13u "$(optionStartMenu)"
  Pop $StartMenuButton
  ${NSD_SetState} $StartMenuButton $ClipStartMenu
  ${NSD_CreateCheckbox} 0 58u 100% 13u "$(optionDesktop)"
  Pop $DesktopButton
  ${NSD_SetState} $DesktopButton $ClipDesktop
  ${NSD_CreateCheckbox} 0 80u 100% 13u "$(optionLaunch)"
  Pop $LaunchButton
  ${NSD_SetState} $LaunchButton $ClipLaunch
  nsDialogs::Show
FunctionEnd

Function OptionsLeave
  ${NSD_GetState} $StartupButton $ClipStartup
  ${NSD_GetState} $StartMenuButton $ClipStartMenu
  ${NSD_GetState} $DesktopButton $ClipDesktop
  ${NSD_GetState} $LaunchButton $ClipLaunch
FunctionEnd

!macro WaitForClipstack executablePath
  ${Do}
    StrCpy $R4 0
    !insertmacro RestartManager_StartSession $R0
    ${If} $R0 != ""
      !insertmacro RestartManager_RegisterFile $R0 "${executablePath}"
      ${If} $0 = 0
        System::Call 'rstrtmgr::RmGetList(p R0, *i 0, *i 0, p 0, *i 0) i .R4'
      ${EndIf}
      !insertmacro RestartManager_EndSession $R0
    ${EndIf}
    ${If} $R4 <> ${ERROR_MORE_DATA}
      ${ExitDo}
    ${EndIf}
    ${If} ${Silent}
      Abort "$(exitRunningApp)"
    ${EndIf}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(exitRunningApp)" IDRETRY +2
    Abort
  ${Loop}
!macroend

Section -Validate
  Call CheckScope
  ${If} $SessionSid != ""
    ReadRegStr $9 HKLM "Software\Microsoft\Windows NT\CurrentVersion\ProfileList\$SessionSid" "ProfileImagePath"
    ${If} $9 != ""
      ExpandEnvStrings $9 $9
      ; The legacy install lived beside its data. Only close the old running
      ; executable here. Personal installation metadata is handled unelevated.
      ${If} ${FileExists} "$9\AppData\Local\${PRODUCTNAME}\${MAINBINARYNAME}.exe"
        !insertmacro WaitForClipstack "$9\AppData\Local\${PRODUCTNAME}\${MAINBINARYNAME}.exe"
      ${EndIf}
    ${EndIf}
  ${EndIf}
  !insertmacro WaitForClipstack "$INSTDIR\${MAINBINARYNAME}.exe"
SectionEnd

; Tauri embeds the Microsoft bootstrapper at build time. An elevated launch
; installs the WebView2 runtime for the machine when it is missing.
Section WebView2
  SetRegView 32
  ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  SetRegView 64
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    InitPluginsDir
    File "/oname=$PLUGINSDIR\MicrosoftEdgeWebView2Setup.exe" "{{webview2_bootstrapper_path}}"
    DetailPrint "$(installingWebview2)"
    ClearErrors
    ExecWait '$"$PLUGINSDIR\MicrosoftEdgeWebView2Setup.exe$" {{webview2_installer_args}} /install' $0
    ${If} ${Errors}
    ${OrIf} $0 <> 0
      Abort "$(webview2AbortError)"
    ${EndIf}
    DetailPrint "$(webview2InstallSuccess)"
  ${EndIf}
SectionEnd

!macro RemoveCommonShortcut path
  ${If} ${FileExists} "${path}"
    !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ShellLink} ${IID_IShellLink} r0 ""
    ${If} $0 P<> 0
      ${IUnknown::QueryInterface} $0 '("${IID_IPersistFile}", .r1)'
      ${If} $1 P<> 0
        ${IPersistFile::Load} $1 '("${path}", ${STGM_READ})'
        ; System allocates the Unicode output buffer for the GetPath argument.
        System::Call '$0->3(w .r2, i ${NSIS_MAX_STRLEN}, p 0, i ${SLGP_RAWPATH}) i .r3'
        ${If} $3 == 0
        ${AndIf} $2 == "$INSTDIR\${MAINBINARYNAME}.exe"
          Delete "${path}"
        ${EndIf}
        ${IUnknown::Release} $1 ""
      ${EndIf}
      ${IUnknown::Release} $0 ""
    ${EndIf}
  ${EndIf}
!macroend

Section Install
  SetOutPath "$INSTDIR"
  ClearErrors
  File "{{main_binary_path}}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  ${If} ${Errors}
    Abort
  ${EndIf}

  ${If} $ClipScope == "CurrentUser"
    !insertmacro RunUserHelper "--configure-install-user $ClipStartup $ClipStartMenu $ClipDesktop $ClipLaunch"
    ${If} $HelperResult != "$HelperToken:ok"
      Abort "$(helperFailed)"
    ${EndIf}
    DeleteRegValue HKLM "${RUNKEY}" "${RUN_ALL}"
    DeleteRegKey HKLM "${UNINSTKEY}"
    !insertmacro RemoveCommonShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    !insertmacro RemoveCommonShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
    WriteRegStr HKLM "${INSTALLKEY}" "OwnerSid" "$SessionSid"
  ${Else}
    WriteRegStr HKLM "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
    WriteRegStr HKLM "${UNINSTKEY}" "DisplayIcon" '$"$INSTDIR\${MAINBINARYNAME}.exe$"'
    WriteRegStr HKLM "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
    WriteRegStr HKLM "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
    WriteRegStr HKLM "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKLM "${UNINSTKEY}" "UninstallString" '$"$INSTDIR\uninstall.exe$"'
    WriteRegDWORD HKLM "${UNINSTKEY}" "NoModify" 1
    WriteRegDWORD HKLM "${UNINSTKEY}" "NoRepair" 1
    DeleteRegValue HKLM "${INSTALLKEY}" "OwnerSid"

    ${If} $ClipStartMenu = ${BST_CHECKED}
      CreateShortCut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${Else}
      !insertmacro RemoveCommonShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}
    ${If} $ClipDesktop = ${BST_CHECKED}
      CreateShortCut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
    ${Else}
      !insertmacro RemoveCommonShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
    ${If} $ClipStartup = ${BST_CHECKED}
      WriteRegStr HKLM "${RUNKEY}" "${RUN_ALL}" '$"$INSTDIR\${MAINBINARYNAME}.exe$" --autostart'
    ${Else}
      DeleteRegValue HKLM "${RUNKEY}" "${RUN_ALL}"
    ${EndIf}

    ${If} $SessionSid != ""
      !insertmacro RunUserHelper "--configure-install-all-user $ClipStartup $ClipLaunch"
      ${If} $HelperResult != "$HelperToken:ok"
        DetailPrint "$(helperFailed)"
        ${IfNot} ${Silent}
          MessageBox MB_ICONEXCLAMATION "$(helperFailed)"
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${EndIf}
  WriteRegStr HKLM "${INSTALLKEY}" "Scope" "$ClipScope"
  WriteRegStr HKLM "${INSTALLKEY}" "InstallLocation" "$INSTDIR"
  ${If} $PassiveMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function un.onInit
  !insertmacro SetContext
  SetShellVarContext all
  StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
  ReadRegStr $ClipScope HKLM "${INSTALLKEY}" "Scope"
  !insertmacro DesktopSid
  ${If} $ClipScope == "CurrentUser"
    ReadRegStr $0 HKLM "${INSTALLKEY}" "OwnerSid"
    ${If} $SessionSid == ""
    ${OrIf} $0 != $SessionSid
      MessageBox MB_ICONEXCLAMATION "$(ownerMismatch)"
      Abort
    ${EndIf}
  ${EndIf}
FunctionEnd

Section Uninstall
  !insertmacro WaitForClipstack "$INSTDIR\${MAINBINARYNAME}.exe"
  ${If} $SessionSid != ""
    !insertmacro RunUserHelper "--remove-install-user"
    ${If} $ClipScope == "CurrentUser"
    ${AndIf} $HelperResult != "$HelperToken:ok"
      Abort "$(helperFailed)"
    ${EndIf}
  ${EndIf}
  DeleteRegValue HKLM "${RUNKEY}" "${RUN_ALL}"
  DeleteRegKey HKLM "${UNINSTKEY}"
  DeleteRegKey HKLM "${INSTALLKEY}"
  !insertmacro RemoveCommonShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  !insertmacro RemoveCommonShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
  Delete /REBOOTOK "$INSTDIR\${MAINBINARYNAME}.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /REBOOTOK "$INSTDIR"
SectionEnd

