# Tests of scripts/install.ps1 (ticket 85), run against a release faked in a folder under `target\`. The
# PowerShell counterpart of scripts/test-install.sh; it runs on Windows only, in CI.
#
# The fake releases folder has the GitHub one's layout -- `download\v<version>\` holding
# `keepster-<version>-x86_64-pc-windows-msvc.zip` (a `keepster-<version>-x86_64-pc-windows-msvc\`
# folder) beside `SHA256SUMS`, and `latest\download\SHA256SUMS`, the latest release's copy -- but its
# `keepster.exe` is a stub compiled here, which prints a version and appends each other command it is
# given to a log. So what is asserted is what the installer does with a release, never what Task
# Scheduler does.
#
# Run under Windows PowerShell 5.1, the one every Windows has and the one `irm | iex` usually meets;
# its Add-Type compiles the stub.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test-install.ps1

$ErrorActionPreference = 'Stop'

$Installer = Join-Path $PSScriptRoot 'install.ps1'
$Version = '0.9.0'
$Target = 'x86_64-pc-windows-msvc'
# Under the build folder rather than TEMP: Windows hands out TEMP by its short 8.3 name on some
# machines, GitHub's runners among them, while a running program reports its own path in full, so the
# path `keepster install` runs from would never compare equal to the one asked for.
$Work = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\target\installer-tests-$([Guid]::NewGuid())"))
New-Item -ItemType Directory -Force -Path $Work | Out-Null

$script:Failures = 0

# Report a failed expectation of the current test; the test goes on, and the run fails at the end.
function Fail([string] $Message) {
    [Console]::Error.WriteLine("  FAIL: $Message")
    $script:Failures++
}

# The stub `keepster.exe`: `--version` prints the faked version, and `sleep` holds the file open as a
# running daemon or tray would. Anything else is appended to KEEPSTER_STUB_LOG as one line, its
# arguments joined by `|`. `install` also writes the path it runs from to `<log>.path`, starts a
# sleeping copy of itself when KEEPSTER_STUB_SPAWN is set -- inheriting its handles, as the tray
# `keepster install` starts does -- and writes that copy's process id to `<log>.spawned`, then exits
# with KEEPSTER_STUB_EXIT (0 by default).
$StubSource = @"
using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
public static class Stub {
    public static int Main(string[] args) {
        if (args.Length > 0 && args[0] == "--version") {
            Console.WriteLine("keepster $Version");
            return 0;
        }
        if (args.Length > 0 && args[0] == "sleep") {
            Thread.Sleep(Timeout.Infinite);
        }
        string log = Environment.GetEnvironmentVariable("KEEPSTER_STUB_LOG");
        if (log != null) {
            File.AppendAllText(log, String.Join("|", args) + Environment.NewLine);
        }
        if (args.Length == 0 || args[0] != "install") {
            return 0;
        }
        string self = Process.GetCurrentProcess().MainModule.FileName;
        if (log != null) {
            File.WriteAllText(log + ".path", self);
        }
        if (Environment.GetEnvironmentVariable("KEEPSTER_STUB_SPAWN") != null) {
            ProcessStartInfo start = new ProcessStartInfo(self, "sleep");
            start.UseShellExecute = false;
            Process spawned = Process.Start(start);
            if (log != null) {
                File.WriteAllText(log + ".spawned", spawned.Id.ToString());
            }
        }
        string exit = Environment.GetEnvironmentVariable("KEEPSTER_STUB_EXIT");
        return exit != null ? Int32.Parse(exit) : 0;
    }
}
"@
$Stub = Join-Path $Work 'stub.exe'
Add-Type -TypeDefinition $StubSource -OutputAssembly $Stub -OutputType ConsoleApplication

# A fresh case folder: `root` the fake releases folder served over file://, `rel` its `$Version`
# release, `latest` where the latest release's SHA256SUMS goes, `tmp` the installer's temporary
# folder, `dest` the install folder (not created), `log` where the stub records what it was given.
function New-Case([string] $Name) {
    $case = @{ Dir = Join-Path $Work $Name }
    $case.Root = Join-Path $case.Dir 'releases'
    $case.Rel = Join-Path $case.Root "download\v$Version"
    $case.Latest = Join-Path $case.Root 'latest\download'
    $case.Tmp = Join-Path $case.Dir 'tmp'
    $case.Dest = Join-Path $case.Dir 'dest'
    $case.Log = Join-Path $case.Dir 'log'
    New-Item -ItemType Directory -Path $case.Rel, $case.Latest, $case.Tmp | Out-Null
    $case
}

# Put the Windows archive into the case's fake `$Version` release, with its line in SHA256SUMS, and
# make that release the latest. With `For`, the archive is for that target instead.
function Add-Release($Case, [string] $For = $Target) {
    $name = "keepster-$Version-$For"
    $dir = Join-Path $Case.Dir "build\$name"
    New-Item -ItemType Directory -Path $dir | Out-Null
    Copy-Item $Stub (Join-Path $dir 'keepster.exe')
    Copy-Item $Stub (Join-Path $dir 'keepster-tray.exe')
    Set-Content -Path (Join-Path $dir 'LICENSE') -Value 'MIT'
    $zip = Join-Path $Case.Rel "$name.zip"
    Compress-Archive -Path $dir -DestinationPath $zip
    $hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
    Add-Content -Path (Join-Path $Case.Rel 'SHA256SUMS') -Value "$hash  $name.zip"
    Copy-Item (Join-Path $Case.Rel 'SHA256SUMS') (Join-Path $Case.Latest 'SHA256SUMS') -Force
}

# Run the installer as a person would, from the case folder, with the case's releases and folders and
# with `Settings` (name to value; $null removes it) over the environment; KEEPSTER_VERSION is
# `$Version` unless a setting removes it. `Arguments` go to the installer; with `Piped`, it is instead
# run as `irm ... | iex` runs it, from its text, followed by one more line of output. Returns the exit
# code (`hung` if it did not end within two minutes), stdout and stderr.
function Invoke-Installer($Case, [hashtable] $Settings = @{}, [string[]] $Arguments = @(), [switch] $Piped) {
    $environment = @{
        KEEPSTER_VERSION = $Version
        KEEPSTER_RELEASES_URL = ([Uri] $Case.Root).AbsoluteUri
        KEEPSTER_INSTALL_DIR = $Case.Dest
        KEEPSTER_STUB_LOG = $Case.Log
        KEEPSTER_STUB_EXIT = $null
        TEMP = $Case.Tmp
        TMP = $Case.Tmp
    }
    foreach ($key in $Settings.Keys) { $environment[$key] = $Settings[$key] }
    $saved = @{}
    foreach ($key in $environment.Keys) {
        $saved[$key] = [Environment]::GetEnvironmentVariable($key)
        [Environment]::SetEnvironmentVariable($key, $environment[$key])
    }
    try {
        if ($Piped) {
            $command = "Get-Content -Raw '$Installer' | Invoke-Expression; Write-Output 'after iex'; exit `$LASTEXITCODE"
            $argumentList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', "`"$command`"")
        } else {
            $quoted = @($Arguments | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } })
            $argumentList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$Installer`"") + $quoted
        }
        $out = Join-Path $Case.Dir 'out'
        $err = Join-Path $Case.Dir 'err'
        # Waited for by hand: `-Wait` would also wait for whatever the run left running.
        $process = Start-Process -FilePath 'powershell' -ArgumentList $argumentList -PassThru -NoNewWindow `
            -WorkingDirectory $Case.Dir -RedirectStandardOutput $out -RedirectStandardError $err
        $null = $process.Handle
        if ($process.WaitForExit(120000)) {
            $code = $process.ExitCode
        } else {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            $code = 'hung'
        }
        @{ Code = $code; Out = (Get-Content -Raw $out) + ''; Err = (Get-Content -Raw $err) + '' }
    } finally {
        foreach ($key in $saved.Keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key]) }
    }
}

# Expect the run to have failed, said `Says` on stderr, and installed nothing.
function Assert-Refused($Case, $Run, [string] $Says) {
    if ($Run.Code -eq 0) { Fail 'it succeeded' }
    if (-not $Run.Err.Contains($Says)) { Fail "stderr does not say '$Says': $($Run.Err)" }
    if (Test-Path $Case.Dest) { Fail "$($Case.Dest) was created" }
    if (Test-Path $Case.Log) { Fail 'keepster was run' }
}

# Expect the run to have succeeded.
function Assert-Succeeded($Run) {
    if ($Run.Code -ne 0) { Fail "exit $($Run.Code): $($Run.Err)" }
}

# Expect the case's install folder to hold the faked release's `keepster.exe`.
function Assert-InstalledThisRelease($Case) {
    $keepster = Join-Path $Case.Dest 'keepster.exe'
    if (-not (Test-Path $keepster) -or (& $keepster --version) -ne "keepster $Version") {
        Fail "the release $Version was not installed"
    }
}

function Test-InstallsBothBinariesSideBySideAndRegistersThem {
    $case = New-Case 'installs'
    Add-Release $case

    $run = Invoke-Installer $case

    Assert-Succeeded $run
    $keepster = Join-Path $case.Dest 'keepster.exe'
    if (-not (Test-Path $keepster)) { Fail "no keepster.exe in $($case.Dest)" }
    if (-not (Test-Path (Join-Path $case.Dest 'keepster-tray.exe'))) { Fail "no keepster-tray.exe in $($case.Dest)" }
    Assert-InstalledThisRelease $case
    if (-not (Test-Path $case.Log) -or (Get-Content $case.Log) -notcontains 'install') { Fail 'keepster install was not run' }
    $ranAs = Get-Content -Raw "$($case.Log).path" -ErrorAction SilentlyContinue
    if ($ranAs -ne $keepster) { Fail "keepster install ran as $ranAs, not from $($case.Dest)" }
}

function Test-WithNoVersionGivenInstallsTheLatestRelease {
    $case = New-Case 'latest'
    Add-Release $case

    $run = Invoke-Installer $case -Settings @{ KEEPSTER_VERSION = $null }

    Assert-Succeeded $run
    Assert-InstalledThisRelease $case
    if (-not (Test-Path $case.Log) -or (Get-Content $case.Log) -notcontains 'install') { Fail 'keepster install was not run' }
}

function Test-RunFromItsTextWithNoVersionGivenAsTheReadmeRunsItInstallsTheLatestRelease {
    $case = New-Case 'piped-latest'
    Add-Release $case

    $run = Invoke-Installer $case -Settings @{ KEEPSTER_VERSION = $null } -Piped

    Assert-Succeeded $run
    Assert-InstalledThisRelease $case
}

function Test-RefusesWhenTheLatestReleaseHasNoBuildForThisPlatformAndNamesIt {
    $case = New-Case 'latest-elsewhere'
    Add-Release $case -For 'aarch64-pc-windows-msvc'

    $run = Invoke-Installer $case -Settings @{ KEEPSTER_VERSION = $null }

    Assert-Refused $case $run $Target
}

function Test-WithNoReleasePublishedNamesWhatCouldNotBeDownloaded {
    $case = New-Case 'no-release'
    Add-Release $case
    Remove-Item (Join-Path $case.Latest 'SHA256SUMS')

    $run = Invoke-Installer $case -Settings @{ KEEPSTER_VERSION = $null }

    Assert-Refused $case $run 'latest/download/SHA256SUMS'
}

function Test-ARelativeInstallFolderIsRegisteredByItsFullPath {
    $case = New-Case 'relative'
    Add-Release $case

    $run = Invoke-Installer $case -Settings @{ KEEPSTER_INSTALL_DIR = 'dest' }

    Assert-Succeeded $run
    $ranAs = Get-Content -Raw "$($case.Log).path" -ErrorAction SilentlyContinue
    if ($ranAs -ne (Join-Path $case.Dest 'keepster.exe')) { Fail "keepster install ran as $ranAs" }
}

function Test-ReturnsWhileATrayKeepsterInstallStartedRunsOn {
    $case = New-Case 'tray-runs-on'
    Add-Release $case

    try {
        $run = Invoke-Installer $case -Settings @{ KEEPSTER_STUB_SPAWN = '1' }

        if ($run.Code -eq 'hung') { Fail 'it waited for the process keepster install started' }
        Assert-Succeeded $run
    } finally {
        $spawned = Get-Content -Raw "$($case.Log).spawned" -ErrorAction SilentlyContinue
        if ($spawned) { Stop-Process -Id ([int] $spawned) -Force -ErrorAction SilentlyContinue }
    }
}

function Test-PassesItsArgumentsToKeepsterInstallAndExitsWithItsCode {
    $case = New-Case 'passes'
    Add-Release $case

    $run = Invoke-Installer $case -Settings @{ KEEPSTER_STUB_EXIT = '7' } -Arguments '--no-tray', '--no-web-ui', 'two words'

    if ($run.Code -ne 7) { Fail "exit $($run.Code), not keepster install's 7" }
    if (-not (Test-Path $case.Log) -or (Get-Content $case.Log) -notcontains 'install|--no-tray|--no-web-ui|two words') {
        Fail "keepster was given: $(Get-Content $case.Log -ErrorAction SilentlyContinue)"
    }
}

function Test-RunFromItsTextAsIrmIexDoesItSetsTheExitCodeAndEndsNoSession {
    $case = New-Case 'piped'
    Add-Release $case

    $run = Invoke-Installer $case -Settings @{ KEEPSTER_STUB_EXIT = '5' } -Piped

    if ($run.Code -ne 5) { Fail "exit $($run.Code), not keepster install's 5: $($run.Err)" }
    if (-not $run.Out.Contains('after iex')) { Fail "the session ended with the script: $($run.Out)" }
    if (-not (Test-Path (Join-Path $case.Dest 'keepster.exe'))) { Fail 'nothing installed' }
}

function Test-RefusesAnArchiveWhoseChecksumDoesNotMatch {
    $case = New-Case 'mismatch'
    Add-Release $case
    Add-Content -Path (Join-Path $case.Rel "keepster-$Version-$Target.zip") -Value 'tampered'

    $run = Invoke-Installer $case

    Assert-Refused $case $run 'checksum'
}

function Test-RefusesAnArchiveThatSha256sumsDoesNotList {
    $case = New-Case 'unlisted'
    Add-Release $case
    Set-Content -Path (Join-Path $case.Rel 'SHA256SUMS') -Value ''

    $run = Invoke-Installer $case

    Assert-Refused $case $run 'SHA256SUMS'
}

function Test-AVersionWithNoReleaseNamesWhatCouldNotBeDownloaded {
    $case = New-Case 'unreleased'
    Add-Release $case

    $run = Invoke-Installer $case -Settings @{ KEEPSTER_VERSION = '0.0.1' }

    Assert-Refused $case $run "keepster-0.0.1-$Target.zip"
}

function Test-RefusesAProcessorWithNoReleaseBuildAndNamesIt {
    $case = New-Case 'arm'
    Add-Release $case

    $run = Invoke-Installer $case -Settings @{ PROCESSOR_ARCHITECTURE = 'ARM64'; PROCESSOR_ARCHITEW6432 = $null }

    Assert-Refused $case $run 'ARM64'
}

function Test-AnUpgradeStopsTheDaemonAndEveryTrayAndReplacesTheBinaries {
    $case = New-Case 'upgrade'
    Add-Release $case
    New-Item -ItemType Directory -Path $case.Dest | Out-Null
    $old = Join-Path $case.Dir 'old.exe'
    Copy-Item $Stub $old
    # An older build: the same stub with one byte more, so it is told apart from the new one.
    [IO.File]::AppendAllText($old, ' ')
    $tray = Join-Path $case.Dest 'keepster-tray.exe'
    Copy-Item $old (Join-Path $case.Dest 'keepster.exe')
    # A tray an earlier upgrade left running: started as `keepster-tray.exe`, then renamed aside.
    Copy-Item $old $tray
    $earlier = Start-Process -FilePath $tray -ArgumentList 'sleep' -PassThru
    Move-Item -LiteralPath $tray -Destination "$tray.old-earlier"
    Copy-Item $old $tray
    $current = Start-Process -FilePath $tray -ArgumentList 'sleep' -PassThru
    try {
        $run = Invoke-Installer $case

        Assert-Succeeded $run
        foreach ($bin in 'keepster.exe', 'keepster-tray.exe') {
            $hash = (Get-FileHash (Join-Path $case.Dest $bin)).Hash
            if ($hash -ne (Get-FileHash $Stub).Hash) { Fail "$bin was not replaced" }
        }
        foreach ($process in $earlier, $current) {
            if (-not $process.HasExited) { Fail "the tray $($process.Id) running from $($case.Dest) was not stopped" }
        }
        $left = @(Get-ChildItem -Name $case.Dest)
        if ($left.Count -ne 2) { Fail "$($case.Dest) holds: $($left -join ', ')" }
        $log = @(Get-Content $case.Log -ErrorAction SilentlyContinue)
        if ($log.Count -lt 2 -or $log[0] -ne 'service|stop' -or $log[1] -ne 'install') {
            Fail "not `service stop` then `install`: $($log -join ' / ')"
        }
    } finally {
        foreach ($process in $earlier, $current) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
    }
}

function Test-ALeftoverFromAnUpgradeGoesOnTheNextRun {
    $case = New-Case 'leftover'
    Add-Release $case
    New-Item -ItemType Directory -Path $case.Dest | Out-Null
    Copy-Item $Stub (Join-Path $case.Dest 'keepster-tray.exe.old-earlier')

    $run = Invoke-Installer $case

    Assert-Succeeded $run
    $left = @(Get-ChildItem -Name $case.Dest)
    if ($left.Count -ne 2 -or $left -notcontains 'keepster.exe' -or $left -notcontains 'keepster-tray.exe') {
        Fail "$($case.Dest) holds: $($left -join ', ')"
    }
}

function Test-AFolderOffPathIsNamedWithTheFullPathToKeepster {
    $case = New-Case 'off-path'
    Add-Release $case

    $run = Invoke-Installer $case

    Assert-Succeeded $run
    if (-not $run.Out.Contains((Join-Path $case.Dest 'keepster.exe'))) { Fail "stdout does not name keepster.exe: $($run.Out)" }
    if (-not $run.Out.Contains('PATH')) { Fail "stdout does not mention PATH: $($run.Out)" }
}

function Test-AFolderOnPathNeedsNoWordAboutIt {
    $case = New-Case 'on-path'
    Add-Release $case

    $run = Invoke-Installer $case -Settings @{ PATH = "$($case.Dest);$env:PATH" }

    Assert-Succeeded $run
    if ($run.Out.Contains('PATH')) { Fail "stdout mentions PATH: $($run.Out)" }
}

function Test-LeavesNoTemporaryFilesWhetherItSucceedsOrFails {
    $case = New-Case 'tidy'
    Add-Release $case

    $run = Invoke-Installer $case
    Assert-Succeeded $run
    if (@(Get-ChildItem -Force $case.Tmp).Count -ne 0) { Fail "a success left $(Get-ChildItem -Name -Force $case.Tmp)" }

    Set-Content -Path (Join-Path $case.Rel 'SHA256SUMS') -Value ''
    $run = Invoke-Installer $case
    if ($run.Code -eq 0) { Fail 'a release with no checksums succeeded' }
    if (@(Get-ChildItem -Force $case.Tmp).Count -ne 0) { Fail "a failure left $(Get-ChildItem -Name -Force $case.Tmp)" }
}

try {
    # The function drive holds only what this session defined, where Get-Command would also find any
    # module's Test-* function.
    foreach ($test in Get-ChildItem 'function:Test-*' | Sort-Object Name) {
        Write-Output $test.Name
        & $test.Name
    }
} finally {
    Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}

if ($script:Failures -ne 0) {
    [Console]::Error.WriteLine("$($script:Failures) expectation(s) failed")
    exit 1
}
Write-Output 'all installer tests passed'
