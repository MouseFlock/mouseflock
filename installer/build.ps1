<#
.SYNOPSIS
Builds DeskHop's Windows Installer package for one architecture.

.DESCRIPTION
From the repository, in any PowerShell (no elevation needed):

    installer\build.ps1 -Arch x64      installer\out\DeskHop-<version>-x64.msi
    installer\build.ps1 -Arch arm64    installer\out\DeskHop-<version>-arm64.msi

Steps (add-installer design D2): the tray app with `tauri build`, the service
and agent with `cargo build --release`, the WebView2 bootstrapper (downloaded
once, its Microsoft signature checked), then the package with `dotnet build`.
The version is the workspace's.

-PackageOnly skips building the programs and packages what was built last.
-OutputSuffix adds text to the file name, for a second package of the same
build (installer\test.ps1 upgrades one with the other). -Version packages
the same programs under another version, to try an upgrade by hand.

Needs: Rust with the target installed, Node.js, the .NET SDK 8 or later, and
for ARM64 the MSVC ARM64 build tools.
#>
param(
    [ValidateSet('x64', 'arm64')]
    [string]$Arch = 'x64',
    [switch]$PackageOnly,
    [string]$OutputSuffix = '',
    # A bootstrapper to use instead of downloading one; it is checked all the same.
    [string]$WebView2Setup = '',
    # A version other than the workspace's, to test an upgrade (for example 0.1.1).
    [string]$Version = ''
)

# Not 'Stop': in Windows PowerShell a native tool's progress on stderr would
# then end the script when output is redirected. Native exit codes are checked
# by Invoke, and our own failures throw.
$ErrorActionPreference = 'Continue'
$Repo = Split-Path -Parent $PSScriptRoot
$Triple = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }[$Arch]
$Platform = @{ x64 = 'x64'; arm64 = 'ARM64' }[$Arch]
$WebView2Url = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703'

function Step([string]$Message) { Write-Host "==> $Message" }

function Invoke([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit code $LASTEXITCODE)" }
}

# Stops unless the file is signed, validly, by Microsoft.
function Assert-MicrosoftSigned([string]$Path) {
    $sig = Get-AuthenticodeSignature -FilePath $Path -ErrorAction Stop
    $subject = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { '' }
    if ($sig.Status -ne 'Valid' -or $subject -notmatch 'O=Microsoft Corporation') {
        throw "$Path is not validly signed by Microsoft (status $($sig.Status), signer '$subject'); not packaging it"
    }
}

function Assert-Arm64Tools {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $found = $false
    if (Test-Path $vswhere) {
        foreach ($vs in & $vswhere -products * -property installationPath) {
            if (Get-ChildItem -Path (Join-Path $vs 'VC\Tools\MSVC\*\lib\arm64\msvcrt.lib') -ErrorAction SilentlyContinue) {
                $found = $true
            }
        }
    }
    if (-not $found) {
        throw ("The MSVC ARM64 libraries are missing. In the Visual Studio Installer, add " +
            "'MSVC v143 - VS 2022 C++ ARM64/ARM64EC build tools' to your Build Tools, or let CI build the ARM64 package.")
    }
}

Push-Location $Repo
try {
    $installed = rustup target list --installed
    if ($installed -notcontains $Triple) {
        throw "The Rust target $Triple is not installed. Run: rustup target add $Triple"
    }
    if ($Arch -eq 'arm64') { Assert-Arm64Tools }

    $metadata = cargo metadata --no-deps --format-version 1 2>$null | ConvertFrom-Json
    if (-not $metadata) { throw 'cargo metadata failed' }
    if (-not $Version) { $Version = ($metadata.packages | Where-Object name -eq 'deskhop-service').version }
    $BinDir = Join-Path $metadata.target_directory "$Triple\release"
    Step "DeskHop $Version for $Arch"

    if (-not $PackageOnly) {
        Step 'tray app (tauri build)'
        Push-Location (Join-Path $Repo 'apps\desktop')
        try {
            if (-not (Test-Path 'node_modules')) { Invoke 'npm ci' { npm ci } }
            Invoke 'tauri build' { npx tauri build --no-bundle --target $Triple }
        } finally { Pop-Location }

        Step 'service and agent (cargo build --release)'
        Invoke 'cargo build' { cargo build --release --locked -p deskhop-service -p deskhop-agent --target $Triple }
    }
    foreach ($bin in 'deskhop-service.exe', 'deskhop-agent.exe', 'DeskHop.exe') {
        if (-not (Test-Path (Join-Path $BinDir $bin))) { throw "$bin is missing from $BinDir" }
    }

    Step 'WebView2 bootstrapper'
    if (-not $WebView2Setup) {
        $obj = Join-Path $PSScriptRoot 'obj\webview2'
        New-Item -ItemType Directory -Force -Path $obj -ErrorAction Stop | Out-Null
        $WebView2Setup = Join-Path $obj 'MicrosoftEdgeWebview2Setup.exe'
        if (-not (Test-Path $WebView2Setup)) {
            Write-Host "    downloading $WebView2Url"
            Invoke-WebRequest -Uri $WebView2Url -OutFile $WebView2Setup -UseBasicParsing -ErrorAction Stop
        }
    }
    Assert-MicrosoftSigned $WebView2Setup
    Write-Host "    $WebView2Setup is signed by Microsoft"

    Step 'package (dotnet build)'
    $project = Join-Path $PSScriptRoot 'DeskHop.wixproj'
    Invoke 'dotnet build' {
        # --no-incremental: every package is linked afresh, with a new product code
        # (D3). Its clean removes what the last build in the same intermediate
        # folder made, so each architecture and suffix has its own.
        dotnet build $project --no-incremental -c Release -nologo -v:minimal "-p:Platform=$Platform" `
            "-p:IntermediateOutputPath=obj\$Arch$OutputSuffix\" "-p:ProductVersion=$Version" `
            "-p:BinDir=$BinDir" "-p:WebView2Setup=$WebView2Setup" "-p:OutputSuffix=$OutputSuffix"
    }
    $msi = Join-Path $PSScriptRoot "out\DeskHop-$Version-$Arch$OutputSuffix.msi"
    Step "built $msi"

    Step 'setup program (dotnet build)'
    $bundle = Join-Path $PSScriptRoot 'DeskHopSetup.wixproj'
    Invoke 'dotnet build (setup)' {
        dotnet build $bundle --no-incremental -c Release -nologo -v:minimal "-p:Platform=$Platform" `
            "-p:IntermediateOutputPath=obj\setup-$Arch$OutputSuffix\" "-p:ProductVersion=$Version" `
            "-p:MsiPath=$msi" "-p:OutputSuffix=$OutputSuffix"
    }
    Step "built $(Join-Path $PSScriptRoot "out\DeskHop-Setup-$Version-$Arch$OutputSuffix.exe")"
} finally {
    Pop-Location
}
