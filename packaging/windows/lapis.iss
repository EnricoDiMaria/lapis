; Inno Setup 6.3+  ->  iscc packaging\windows\lapis.iss /DAppVersion=1.1.0
#define AppName "Lapis"
#ifndef AppVersion
  #define AppVersion "1.1.0"
#endif
#define AppExe "lapis.exe"

[Setup]
AppId={{6F3B1C2E-7A44-4D8B-9E15-2C0A5B7D9F31}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Enrico Di Maria
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
SetupIconFile=..\..\assets\lapis.ico
OutputDir=..\..\dist
OutputBaseFilename=lapis-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/ultra
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; installazione per-utente di default (nessun UAC); l'utente può scegliere "per tutti" dal dialog
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
WizardStyle=modern

[Languages]
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Icona sul desktop"; Flags: unchecked
Name: "pdfassoc"; Description: "Imposta Lapis come lettore PDF predefinito (consigliato: scegli comunque da Impostazioni di Windows)"; Flags: unchecked
Name: "downloadmodel"; Description: "Scarica il modello di trascrizione offline Parakeet (~650 MB)"; Flags: unchecked
; ffmpeg non è incluso nell'installer: viene scaricato dal sito ufficiale (solo se non è già nel PATH)
Name: "downloadffmpeg"; Description: "Scarica ffmpeg (serve per importare audio/video, cambiare velocità e convertire le registrazioni)"; Check: not HasFFmpeg

[Files]
Source: "..\..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
; script che scarica VC++ Runtime e ffmpeg dai siti ufficiali (nulla di terze parti è incluso nell'installer)
Source: "install-deps.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; "Apri con" > Lapis
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""
; associazione opzionale
Root: HKA; Subkey: "Software\Classes\Lapis.pdf"; ValueType: string; ValueData: "PDF (Lapis)"; Flags: uninsdeletekey; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Lapis.pdf\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "Lapis.pdf"; ValueData: ""; Flags: uninsdeletevalue; Tasks: pdfassoc

[Run]
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\install-deps.ps1"" -VCRedist"; StatusMsg: "Download e installazione Microsoft Visual C++ Runtime..."; Flags: waituntilterminated; Check: NeedsVCRedist
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\install-deps.ps1"" -FFmpeg -AppDir ""{app}"""; StatusMsg: "Download di ffmpeg..."; Flags: waituntilterminated; Tasks: downloadffmpeg
Filename: "{app}\{#AppExe}"; Description: "Avvia Lapis"; Flags: nowait postinstall skipifsilent

[Code]
const
  ModelBase = 'https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/';

var
  ModelPage: TDownloadWizardPage;
  ModelOk: Boolean;

function HasFFmpeg: Boolean;
begin
  Result := FileSearch('ffmpeg.exe', GetEnv('PATH')) <> '';
end;

function NeedsVCRedist: Boolean;
var
  Installed: Cardinal;
begin
  Result := not (RegQueryDWordValue(HKLM64, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Installed', Installed) and (Installed = 1));
end;

procedure InitializeWizard;
begin
  ModelPage := CreateDownloadPage('Modello di trascrizione', 'Download del modello Parakeet TDT 0.6B v3 (int8)...', nil);
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if (CurPageID = wpReady) and WizardIsTaskSelected('downloadmodel') then
  begin
    ModelPage.Clear;
    { Inserire gli SHA256 reali al posto di '' per la verifica d'integrità }
    ModelPage.Add(ModelBase + 'vocab.txt', 'vocab.txt', '');
    ModelPage.Add(ModelBase + 'encoder-model.int8.onnx', 'encoder-model.int8.onnx', '');
    ModelPage.Add(ModelBase + 'decoder_joint-model.int8.onnx', 'decoder_joint-model.int8.onnx', '');
    ModelPage.Show;
    try
      try
        ModelPage.Download;
        ModelOk := True;
      except
        ModelOk := False;
        { non bloccante: si può riprovare dopo con  install.ps1 model }
        SuppressibleMsgBox('Download del modello non riuscito. Lapis verrà installato comunque; potrai scaricare il modello più tardi.', mbInformation, MB_OK, IDOK);
      end;
    finally
      ModelPage.Hide;
    end;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Dest: String;
begin
  if (CurStep = ssPostInstall) and ModelOk then
  begin
    Dest := ExpandConstant('{localappdata}\lapis\models\parakeet-tdt-0.6b-v3-int8');
    ForceDirectories(Dest);
    FileCopy(ExpandConstant('{tmp}\vocab.txt'), Dest + '\vocab.txt', False);
    FileCopy(ExpandConstant('{tmp}\encoder-model.int8.onnx'), Dest + '\encoder-model.int8.onnx', False);
    FileCopy(ExpandConstant('{tmp}\decoder_joint-model.int8.onnx'), Dest + '\decoder_joint-model.int8.onnx', False);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    if MsgBox('Rimuovere anche modelli ASR, cache audio, dizionari e impostazioni di Lapis?', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    begin
      DelTree(ExpandConstant('{localappdata}\lapis'), True, True, True);
      DelTree(ExpandConstant('{userappdata}\lapis'), True, True, True);
    end;
end;
