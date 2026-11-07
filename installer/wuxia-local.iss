; Inno Setup script for Wuxia Local. Built by CI:
;   ISCC.exe /DAppVersion=0.1.0 /DDist=<path to dist> wuxia-local.iss
; Installs per-user (no admin rights) into %LOCALAPPDATA%\Programs\WuxiaLocal.

#ifndef AppVersion
  #define AppVersion "0.2.0"
#endif
#ifndef Dist
  #define Dist "..\dist"
#endif

[Setup]
AppId={{6B1E2C4A-7F3D-4C3B-9A51-5D0E2A9C1F77}
AppName=Wuxia Local
AppVersion={#AppVersion}
AppPublisher=Wuxia Local contributors
DefaultDirName={localappdata}\Programs\WuxiaLocal
DefaultGroupName=Wuxia Local
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
OutputBaseFilename=WuxiaLocal-Setup-{#AppVersion}-x64
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\wuxia-local.exe
SetupIconFile=..\assets\icon.ico
DisableProgramGroupPage=yes
; Close a running copy before replacing files (otherwise the old exe stays locked).
CloseApplications=force
RestartApplications=no

[Languages]
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[Types]
Name: "gpu"; Description: "Видеокарта (Vulkan) — рекомендуется"
Name: "cpu"; Description: "Только процессор (старые ПК без поддержки Vulkan)"

[Components]
Name: "app"; Description: "Wuxia Local"; Types: gpu cpu; Flags: fixed
Name: "vulkan"; Description: "Нейросетевой движок llama.cpp (Vulkan: NVIDIA / AMD / Intel)"; Types: gpu; Flags: exclusive
Name: "cpu"; Description: "Нейросетевой движок llama.cpp (только CPU)"; Types: cpu; Flags: exclusive

[Tasks]
Name: "desktopicon"; Description: "Ярлык на рабочем столе"
Name: "ocr"; Description: "Установить модули распознавания текста Windows — английский и китайский (нужны права администратора, ~20 МБ)"

[InstallDelete]
; Renamed in 0.2 to wwm_wuxia_common.tsv (loaded only for Where Winds Meet).
Type: files; Name: "{app}\glossary\00_wuxia_common.tsv"

[Files]
Source: "{#Dist}\wuxia-local.exe"; DestDir: "{app}"; Components: app; Flags: ignoreversion
Source: "{#Dist}\config.default.toml"; DestDir: "{app}"; Components: app; Flags: ignoreversion
Source: "{#Dist}\README.md"; DestDir: "{app}"; Components: app; Flags: ignoreversion
Source: "{#Dist}\LICENSE"; DestDir: "{app}"; Components: app; Flags: ignoreversion
Source: "{#Dist}\LLAMA_VERSION.txt"; DestDir: "{app}"; Components: app; Flags: ignoreversion
Source: "{#Dist}\glossary\*"; DestDir: "{app}\glossary"; Components: app; Flags: ignoreversion recursesubdirs
Source: "{#Dist}\bench\*"; DestDir: "{app}\bench"; Components: app; Flags: ignoreversion recursesubdirs
Source: "{#Dist}\llama-vulkan\*"; DestDir: "{app}\llama"; Components: vulkan; Flags: ignoreversion recursesubdirs
Source: "{#Dist}\llama-cpu\*"; DestDir: "{app}\llama"; Components: cpu; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\Wuxia Local"; Filename: "{app}\wuxia-local.exe"
Name: "{group}\Папка данных Wuxia Local"; Filename: "{localappdata}\WuxiaLocal"
Name: "{autodesktop}\Wuxia Local"; Filename: "{app}\wuxia-local.exe"; Tasks: desktopicon

[Run]
; Windows OCR language components (en-US, zh-CN). Run elevated in the background; no PowerShell knowledge needed.
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command ""Add-WindowsCapability -Online -Name 'Language.OCR~~~en-US~0.0.1.0'; Add-WindowsCapability -Online -Name 'Language.OCR~~~zh-CN~0.0.1.0'"""; Verb: runas; Flags: shellexec waituntilterminated; Tasks: ocr; StatusMsg: "Установка модулей распознавания текста Windows…"
Filename: "{app}\wuxia-local.exe"; Description: "Запустить Wuxia Local"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "taskkill.exe"; Parameters: "/F /IM llama-server.exe"; Flags: runhidden; RunOnceId: "killllama"

[Code]
// Close a running Wuxia Local before files are replaced; its llama-server child
// exits with it (job object). Without this the old exe stayed locked.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM wuxia-local.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(800);
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    if MsgBox('Удалить также скачанные модели, кэш переводов, исправления и настройки?' + #13#10 +
              '(' + ExpandConstant('{localappdata}\WuxiaLocal') + ')', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{localappdata}\WuxiaLocal'), True, True, True);
end;
