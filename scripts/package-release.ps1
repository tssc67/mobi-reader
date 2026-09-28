param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot
try {
    $metadataJson = & cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $metadata = $metadataJson | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.name -eq 'mobi-reader' }
    $releaseName = "mobi-reader-v$($package.version)-windows-x86_64"
    $hostInfo = & rustc -vV
    if ($hostInfo -notcontains 'host: x86_64-pc-windows-msvc') {
        throw 'Package releases on an x86_64-pc-windows-msvc host.'
    }
    if (-not $SkipBuild) {
        & cargo build --locked --release
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    }
    $binary = Join-Path $metadata.target_directory 'release/mobi-reader.exe'
    if (-not (Test-Path -LiteralPath $binary)) { throw "Missing release binary: $binary" }
    $stage = Join-Path $projectRoot "dist/$releaseName"
    if (Test-Path -LiteralPath $stage) { throw "Release staging directory already exists: $stage" }
    New-Item -ItemType Directory -Path "$stage/assets/fonts", "$stage/vendor/blitz-dom" -Force | Out-Null
    Copy-Item -LiteralPath $binary -Destination "$stage/mobi-reader.exe"
    foreach ($file in @('README.md', 'LICENSE', 'CHANGELOG.md', 'CONTRIBUTING.md')) {
        Copy-Item -LiteralPath $file -Destination $stage
    }
    Copy-Item -LiteralPath 'docs' -Destination "$stage/docs" -Recurse
    foreach ($file in @('SourceSerif4-LICENSE.md', 'SourceSans3-LICENSE.md')) {
        Copy-Item -LiteralPath "assets/fonts/$file" -Destination "$stage/assets/fonts/$file"
    }
    foreach ($file in @('README.md', 'LICENSE-MIT', 'LICENSE-APACHE')) {
        Copy-Item -LiteralPath "vendor/blitz-dom/$file" -Destination "$stage/vendor/blitz-dom/$file"
    }
    $archive = Join-Path $projectRoot "dist/$releaseName.zip"
    Compress-Archive -LiteralPath $stage -DestinationPath $archive
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $releaseName.zip" | Set-Content -LiteralPath "$archive.sha256" -Encoding ascii
    Write-Host "Created $archive"
}
finally { Pop-Location }
