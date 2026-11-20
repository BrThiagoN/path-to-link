$ErrorActionPreference = 'Stop'

$installerPath = Join-Path $PSScriptRoot '..\install.ps1'
$tokens = $null
$parseErrors = $null
$parser = [System.Management.Automation.Language.Parser]
$null = $parser::ParseFile($installerPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -gt 0) {
    throw "install.ps1 has PowerShell syntax errors: $($parseErrors -join '; ')"
}

$installerAst = [System.Management.Automation.Language.Parser]::ParseFile(
    $installerPath,
    [ref]$tokens,
    [ref]$parseErrors
)
$helperAst = $installerAst.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Get-ExpectedSha256'
}, $true)
if ($null -eq $helperAst) {
    throw 'Get-ExpectedSha256 was not found in install.ps1.'
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
        [string]$ExpectedMessage,
        [string]$CaseName
    )

    try {
        $null = Get-ExpectedSha256 -ChecksumText $ChecksumText -AssetName 'path-to-link.exe'
    } catch {
        if ($_.Exception.Message -like $ExpectedMessage) {
            return
        }
        throw "$CaseName failed with an unexpected error: $($_.Exception.Message)"
    }

    throw "$CaseName failed. Expected an exception."
}

$digest = 'A' * 64
$parsed = Get-ExpectedSha256 -ChecksumText "$digest  path-to-link.exe`r`n" -AssetName 'path-to-link.exe'
Assert-Equal -Expected ('a' * 64) -Actual $parsed -CaseName 'Valid checksum is normalized'

Assert-Throws -ChecksumText $null -ExpectedMessage '*is empty*' -CaseName 'Null checksum is rejected'
Assert-Throws -ChecksumText "`r`n  `t" -ExpectedMessage '*is empty*' -CaseName 'Whitespace checksum is rejected'
Assert-Throws -ChecksumText 'not-a-sha256' -ExpectedMessage '*is invalid*' -CaseName 'Malformed checksum is rejected'

Write-Output 'Passed 4 Windows installer checksum cases.'
