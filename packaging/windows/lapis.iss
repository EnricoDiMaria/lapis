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
; per-user install by default (no UAC); the user can choose "all users" in the dialog
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
WizardStyle=modern
; update in place: same AppId = same install folder, shortcuts and uninstall entry.
; A running Lapis is closed (and not restarted) before its files are replaced.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop icon"; Flags: unchecked
Name: "pdfassoc"; Description: "Set Lapis as the default PDF reader (you can still change this in Windows Settings)"; Flags: unchecked
Name: "downloadmodel"; Description: "Download the offline Parakeet transcription model (~650 MB)"; Flags: unchecked dontinheritcheck; Check: not HasModel
; ffmpeg is not bundled: it is downloaded from the official site (only if it is not already in PATH)
Name: "downloadffmpeg"; Description: "Download ffmpeg (needed to import audio/video, change playback speed and convert recordings)"; Check: not HasFFmpeg

[Files]
Source: "..\..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
; script that downloads the VC++ Runtime and ffmpeg from their official sites (no third-party binaries are bundled)
Source: "install-deps.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; "Open with" > Lapis
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""
; optional file association
Root: HKA; Subkey: "Software\Classes\Lapis.pdf"; ValueType: string; ValueData: "PDF (Lapis)"; Flags: uninsdeletekey; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Lapis.pdf\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "Lapis.pdf"; ValueData: ""; Flags: uninsdeletevalue; Tasks: pdfassoc

[UninstallDelete]
; ffmpeg.exe is downloaded after installation, so Setup does not track it: without this line
; it stays behind and the (non-empty) install folder is not removed.
Type: files; Name: "{app}\ffmpeg.exe"
Type: dirifempty; Name: "{app}"

[Run]
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\install-deps.ps1"" -VCRedist"; StatusMsg: "Downloading and installing the Microsoft Visual C++ Runtime..."; Flags: waituntilterminated; Check: NeedsVCRedist
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\install-deps.ps1"" -FFmpeg -AppDir ""{app}"""; StatusMsg: "Downloading ffmpeg..."; Flags: waituntilterminated; Tasks: downloadffmpeg
Filename: "{app}\{#AppExe}"; Description: "Launch Lapis"; Flags: nowait postinstall skipifsilent

[Code]
const
  ModelBase = 'https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/';

var
  ModelPage: TDownloadWizardPage;
  ModelOk: Boolean;

const
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{6F3B1C2E-7A44-4D8B-9E15-2C0A5B7D9F31}_is1';

{ Reads a value of the existing Lapis installation (per-user or all-users), if any. }
function ReadInstalled(const ValueName: String; var Value: String): Boolean;
begin
  Result := RegQueryStringValue(HKCU, UninstallKey, ValueName, Value) or
            RegQueryStringValue(HKLM, UninstallKey, ValueName, Value);
end;

{ Takes the next numeric part of a dotted version ("1.2.0" -> 1, leaving "2.0"). }
function NextVersionPart(var S: String): Integer;
var
  P: Integer;
begin
  P := Pos('.', S);
  if P = 0 then
  begin
    Result := StrToIntDef(S, 0);
    S := '';
  end
  else
  begin
    Result := StrToIntDef(Copy(S, 1, P - 1), 0);
    Delete(S, 1, P);
  end;
end;

{ -1 if A < B, 0 if equal, 1 if A > B. A suffix such as "-rc1" is ignored. }
function CompareVersionStrings(A, B: String): Integer;
var
  P, I, PartA, PartB: Integer;
begin
  Result := 0;
  P := Pos('-', A);
  if P > 0 then A := Copy(A, 1, P - 1);
  P := Pos('-', B);
  if P > 0 then B := Copy(B, 1, P - 1);
  for I := 1 to 4 do
  begin
    PartA := NextVersionPart(A);
    PartB := NextVersionPart(B);
    if PartA < PartB then
    begin
      Result := -1;
      Exit;
    end;
    if PartA > PartB then
    begin
      Result := 1;
      Exit;
    end;
  end;
end;

{ Asks before replacing a newer version with an older one (silent installs refuse). }
function InitializeSetup: Boolean;
var
  Installed: String;
begin
  Result := True;
  if ReadInstalled('DisplayVersion', Installed) then
    if CompareVersionStrings(Installed, '{#AppVersion}') > 0 then
      Result := SuppressibleMsgBox('A newer version of Lapis (' + Installed + ') is already installed.' + #13#10#13#10 +
        'Do you want to replace it with the older version {#AppVersion}?',
        mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES;
end;

{ ffmpeg is "present" if it is in PATH or next to an existing Lapis installation (update case). }
function HasFFmpeg: Boolean;
var
  Dir: String;
begin
  Result := FileSearch('ffmpeg.exe', GetEnv('PATH')) <> '';
  if (not Result) and ReadInstalled('InstallLocation', Dir) then
    Result := FileExists(AddBackslash(Dir) + 'ffmpeg.exe');
end;

{ The transcription model is already installed (update case): do not offer the 650 MB download again. }
function HasModel: Boolean;
begin
  Result := FileExists(ExpandConstant('{localappdata}\lapis\models\parakeet-tdt-0.6b-v3-int8\encoder-model.int8.onnx'));
end;

function NeedsVCRedist: Boolean;
var
  Installed: Cardinal;
begin
  Result := not (RegQueryDWordValue(HKLM64, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Installed', Installed) and (Installed = 1));
end;

procedure InitializeWizard;
begin
  ModelPage := CreateDownloadPage('Transcription model', 'Downloading the Parakeet TDT 0.6B v3 (int8) model...', nil);
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if (CurPageID = wpReady) and WizardIsTaskSelected('downloadmodel') then
  begin
    ModelPage.Clear;
    { Put the real SHA256 hashes in place of '' to verify file integrity }
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
        { not blocking: the model can be added later }
        SuppressibleMsgBox('The model download failed. Lapis will be installed anyway; you can add the model later by placing its files in %LOCALAPPDATA%\lapis\models\parakeet-tdt-0.6b-v3-int8.', mbInformation, MB_OK, IDOK);
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
    if MsgBox('Also remove the data Lapis keeps on this computer: ASR models, audio recordings, notes backups, dictionaries and settings?' + #13#10#13#10 + 'Notes saved inside your PDF files are not affected.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    begin
      DelTree(ExpandConstant('{localappdata}\lapis'), True, True, True);
      DelTree(ExpandConstant('{userappdata}\lapis'), True, True, True);
    end;
end;
