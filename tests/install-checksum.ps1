$ErrorActionPreference = 'Stop'

$installerPath = Join-Path $PSScriptRoot '..\install.ps1'
$tokens = $null
$parseErrors = $null
$parser = [System.Management.Automation.Language.Parser]
$installerAst = $parser::ParseFile($installerPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -gt 0) {
    throw "install.ps1 has PowerShell syntax errors: $($parseErrors -join '; ')"
}

$helperAst = $installerAst.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Get-ExpectedSha256'
}, $true)
if ($null -eq $helperAst) {
    throw 'Get-ExpectedSha256 was not found in install.ps1.'
}
$helperCallAst = $installerAst.Find({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
        $node.GetCommandName() -eq 'Get-ExpectedSha256'
}, $true)
if ($null -eq $helperCallAst) {
    throw 'install.ps1 does not use Get-ExpectedSha256 to validate the downloaded checksum.'
}
. ([scriptblock]::Create($helperAst.Extent.Text))

function Assert-Equal {
    param([string]$Expected, [string]$Actual, [string]$CaseName)
    if ($Expected -cne $Actual) {
        throw "$CaseName failed. Expected '$Expected'; received '$Actual'."
    }
}

function Assert-Throws {
    param(
        [AllowNull()][string]$ChecksumText,
        [string]$CaseName
    )

    try {
        $null = Get-ExpectedSha256 -ChecksumText $ChecksumText -AssetName 'path-to-link.exe'
    } catch {
        if ($_.Exception -is [System.IO.InvalidDataException]) {
            return
        }
        throw "$CaseName failed with an unexpected error: $($_.Exception.Message)"
    }

    throw "$CaseName failed. Expected an exception."
}

$digest = 'A' * 64
$parsed = Get-ExpectedSha256 -ChecksumText "$digest  path-to-link.exe`r`n" -AssetName 'path-to-link.exe'
Assert-Equal -Expected ('a' * 64) -Actual $parsed -CaseName 'Valid checksum is normalized'

Assert-Throws -ChecksumText $null -CaseName 'Null checksum is rejected'
Assert-Throws -ChecksumText "`r`n  `t" -CaseName 'Whitespace checksum is rejected'
Assert-Throws -ChecksumText 'not-a-sha256' -CaseName 'Malformed checksum is rejected'

Write-Output 'Passed 5 Windows installer checksum checks.'
