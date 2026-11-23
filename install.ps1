$ErrorActionPreference = 'Stop'

function Get-ExpectedSha256 {
    param(
        [AllowNull()][string]$ChecksumText,
        [Parameter(Mandatory)][string]$AssetName
    )

    if ([string]::IsNullOrWhiteSpace($ChecksumText)) {
        throw [System.IO.InvalidDataException]::new("The SHA-256 file for $AssetName is empty. The release may be incomplete; try again later.")
    }

    $expected = ($ChecksumText.Trim() -split '\s+')[0]
    if ($expected -notmatch '^[0-9a-fA-F]{64}$') {
        throw [System.IO.InvalidDataException]::new("The SHA-256 file for $AssetName is invalid. The release may be incomplete; try again later.")
    }

    return $expected.ToLowerInvariant()
}

$repository = 'BrThiagoN/path-to-link'
$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()

switch ($architecture) {
    'X64'   { $target = 'x86_64-pc-windows-msvc' }
    'Arm64' { $target = 'aarch64-pc-windows-msvc' }
    default { throw "Unsupported Windows architecture: $architecture" }
}

$assetName = "path-to-link-$target.exe"
$headers = @{
    Accept = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
}

try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repository/releases/latest" -Headers $headers
} catch {
    throw 'Could not find a published Path-to-Link release. Install from source with: cargo install --git https://github.com/BrThiagoN/path-to-link'
}

$asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
$checksumAsset = $release.assets | Where-Object { $_.name -eq "$assetName.sha256" } | Select-Object -First 1
if ($null -eq $asset -or $null -eq $checksumAsset) {
    throw "The latest release does not include a build for $architecture."
}

$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) "path-to-link-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $tempDir | Out-Null

try {
    $downloadPath = Join-Path $tempDir $assetName
    $checksumPath = Join-Path $tempDir "$assetName.sha256"
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $downloadPath -UseBasicParsing
    Invoke-WebRequest -Uri $checksumAsset.browser_download_url -OutFile $checksumPath -UseBasicParsing

    $checksumText = Get-Content -LiteralPath $checksumPath -Raw
    $expected = Get-ExpectedSha256 -ChecksumText $checksumText -AssetName $assetName
    $actual = (Get-FileHash -LiteralPath $downloadPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw 'The downloaded file failed its SHA-256 check.'
    }

    $installDir = Join-Path $env:LOCALAPPDATA 'Programs\Path-to-Link'
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    $executablePath = Join-Path $installDir 'ptl.exe'
    Copy-Item -LiteralPath $downloadPath -Destination $executablePath -Force
    Copy-Item -LiteralPath $downloadPath -Destination (Join-Path $installDir 'path-to-link.exe') -Force

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    if ($pathEntries -notcontains $installDir) {
        $updatedPath = (@($pathEntries) + $installDir) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $updatedPath, 'User')
    }

    if (($env:Path -split ';') -notcontains $installDir) {
        $env:Path = "$installDir;$env:Path"
    }

    Write-Host "Installed Path-to-Link to $installDir"
    Write-Host 'Run: ptl help'
    & $executablePath --version
} finally {
    Remove-Item -LiteralPath $tempDir -Recurse -Force
}
