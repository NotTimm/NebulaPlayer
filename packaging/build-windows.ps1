# Builds the Windows packages from one staged folder (nebula-player.exe,
# libmpv-2.dll, which it loads to play videos, and vulkan\vulkan-1.dll,
# the Vulkan loader libmpv needs, used only when Windows has none):
#   - a portable .zip: unzip anywhere and run
#   - a per-user installer (Inno Setup, packaging/windows/nebula-player.iss).
#     Skipped with a warning if Inno Setup isn't installed, unless
#     REQUIRE_INSTALLER=1 (as in CI).
#
# Needs Rust (MSVC toolchain). libmpv-2.dll comes from MPV_DLL if set,
# else from the pinned mpv build below (downloaded once into dist\, checked
# by hash, unpacked with 7-Zip); the Vulkan loader likewise from NuGet.
$ErrorActionPreference = 'Stop'

# One exact libmpv build, so a new upload can't change what ships. Newer:
# https://sourceforge.net/projects/mpv-player-windows/files/libmpv/
$MpvDevFile = 'mpv-dev-x86_64-20261018-git-01e2533f31.7z'
$MpvDevSha256 = 'd7f5621f42b6e161fe032df914a10bdece28a89df3e57c6c3e16a5a250929b5e'

# Khronos' Vulkan loader (Apache-2.0, 1.3.283), as built for Silk.NET.
# libmpv-2.dll imports vulkan-1.dll; GPU drivers install it, but machines
# without one (VMs, CI) couldn't load libmpv at all.
$VulkanPkg = 'silk.net.vulkan.loader.native.2025.9.12.nupkg'
$VulkanPkgSha256 = '33811c05ab0bcba632ad38abba459b0d72d6e58859342e0782d011863dde07d1'

$Repo = Resolve-Path (Join-Path $PSScriptRoot '..')
$Dist = Join-Path $Repo 'dist'
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

# Version comes from Cargo.toml's [package] table, like the Linux scripts.
$Version = $null
$InPackage = $false
foreach ($line in Get-Content (Join-Path $Repo 'Cargo.toml')) {
    if ($line -match '^\[') { $InPackage = ($line -eq '[package]'); continue }
    if ($InPackage -and $line -match '^version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
}
if (-not $Version) { throw 'could not read version from Cargo.toml' }

# 7-Zip unpacks the libmpv and Vulkan loader downloads.
$SevenZip = (Get-Command 7z -ErrorAction SilentlyContinue).Source
if (-not $SevenZip) { $SevenZip = "$env:ProgramFiles\7-Zip\7z.exe" }
if (-not (Test-Path $SevenZip)) { throw '7-Zip not found (winget install 7zip.7zip)' }

# libmpv
$MpvDll = $env:MPV_DLL
if (-not $MpvDll) {
    $Archive = Join-Path $Dist $MpvDevFile
    if (-not (Test-Path $Archive)) {
        Write-Host "==> Downloading $MpvDevFile"
        $Url = "https://sourceforge.net/projects/mpv-player-windows/files/libmpv/$MpvDevFile/download"
        curl.exe -fsSL --retry 5 -o $Archive $Url
        if ($LASTEXITCODE -ne 0) { throw "couldn't download $Url" }
    }
    $Hash = (Get-FileHash $Archive -Algorithm SHA256).Hash.ToLower()
    if ($Hash -ne $MpvDevSha256) {
        Remove-Item $Archive
        throw "$MpvDevFile has hash $Hash, expected $MpvDevSha256"
    }
    $MpvDir = Join-Path $Dist 'mpv-dev'
    & $SevenZip x -y "-o$MpvDir" $Archive libmpv-2.dll | Out-Null
    if ($LASTEXITCODE -ne 0) { throw '7-Zip failed' }
    $MpvDll = Join-Path $MpvDir 'libmpv-2.dll'
}
if (-not (Test-Path $MpvDll)) { throw "libmpv-2.dll not found at $MpvDll" }

# Vulkan loader
$VulkanArchive = Join-Path $Dist $VulkanPkg
if (-not (Test-Path $VulkanArchive)) {
    Write-Host "==> Downloading $VulkanPkg"
    $Url = "https://api.nuget.org/v3-flatcontainer/silk.net.vulkan.loader.native/2025.9.12/$VulkanPkg"
    curl.exe -fsSL --retry 5 -o $VulkanArchive $Url
    if ($LASTEXITCODE -ne 0) { throw "couldn't download $Url" }
}
$Hash = (Get-FileHash $VulkanArchive -Algorithm SHA256).Hash.ToLower()
if ($Hash -ne $VulkanPkgSha256) {
    Remove-Item $VulkanArchive
    throw "$VulkanPkg has hash $Hash, expected $VulkanPkgSha256"
}
$VulkanDir = Join-Path $Dist 'vulkan-loader'
& $SevenZip e -y "-o$VulkanDir" $VulkanArchive 'runtimes/win-x64/native/vulkan-1.dll' | Out-Null
if ($LASTEXITCODE -ne 0) { throw '7-Zip failed' }
$VulkanDll = Join-Path $VulkanDir 'vulkan-1.dll'
if (-not (Test-Path $VulkanDll)) { throw "vulkan-1.dll not found in $VulkanPkg" }

Write-Host '==> Building release binary'
Push-Location $Repo
try {
    $flags = @('build', '--release')
    if ($env:CARGO_BUILD_FLAGS) { $flags += $env:CARGO_BUILD_FLAGS.Split(' ') }
    cargo @flags
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally {
    Pop-Location
}

$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo 'target' }
$Exe = Join-Path $TargetDir 'release\nebula-player.exe'

$Name = "NebulaPlayer-$Version-windows-x64"
$Stage = Join-Path ([System.IO.Path]::GetTempPath()) $Name
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Path $Stage | Out-Null

Copy-Item $Exe $Stage
Copy-Item $MpvDll $Stage
$StageVulkan = Join-Path $Stage 'vulkan'
New-Item -ItemType Directory -Path $StageVulkan | Out-Null
Copy-Item $VulkanDll $StageVulkan
@"
vulkan-1.dll is Khronos' Vulkan loader (https://github.com/KhronosGroup/Vulkan-Loader),
licensed under the Apache License 2.0 (https://www.apache.org/licenses/LICENSE-2.0).
Nebula Player uses it only when Windows has no Vulkan loader of its own.
"@ | Set-Content -Encoding utf8 (Join-Path $StageVulkan 'README.txt')
Copy-Item (Join-Path $Repo 'LICENSE') (Join-Path $Stage 'LICENSE.txt')
@"
Nebula Player $Version for Windows (portable)

Run nebula-player.exe. Keep libmpv-2.dll and the vulkan folder next to
it: they play the videos.

Your settings and Jellyfin sign-ins are stored in %APPDATA%\NebulaPlayer
(mpv.conf and input.conf go in its mpv folder) and the poster cache in
%LOCALAPPDATA%\NebulaPlayer\Cache, so you can replace this folder with a
newer version without losing anything.

https://github.com/NotTimm/NebulaPlayer
"@ | Set-Content -Encoding utf8 (Join-Path $Stage 'README.txt')

# The exe finds libmpv-2.dll beside it and loads it.
$Check = Start-Process (Join-Path $Stage 'nebula-player.exe') -ArgumentList '--check-mpv' -Wait -PassThru -NoNewWindow
if ($Check.ExitCode -ne 0) { throw "nebula-player.exe --check-mpv exited with $($Check.ExitCode)" }
Write-Host '==> libmpv-2.dll loads'

$Zip = Join-Path $Dist "$Name.zip"
if (Test-Path $Zip) { Remove-Item $Zip }
Compress-Archive -Path $Stage -DestinationPath $Zip
Write-Host "==> $Zip"

# Installer. ISCC is Inno Setup's command-line compiler.
$Iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $Iscc) {
    $Iscc = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if ($Iscc) {
    # Windows file versions are numeric: 0.1.0-alpha.1 -> 0.1.0
    $Numeric = ($Version -split '-')[0]
    & $Iscc /Q "/DAppVersion=$Version" "/DNumericVersion=$Numeric" `
        "/DStageDir=$Stage" "/DOutputDir=$Dist" `
        (Join-Path $PSScriptRoot 'windows\nebula-player.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
    Write-Host "==> $(Join-Path $Dist "$Name-setup.exe")"
} elseif ($env:REQUIRE_INSTALLER -eq '1') {
    throw 'Inno Setup (ISCC.exe) not found; install it with: winget install JRSoftware.InnoSetup'
} else {
    Write-Warning 'Inno Setup not found; skipping the installer (winget install JRSoftware.InnoSetup)'
}

Remove-Item -Recurse -Force $Stage
