# Install Keepster from a GitHub release on Windows (ticket 85):
#
#   irm https://github.com/kravlab/keepster/releases/latest/download/install.ps1 | iex
#
# The PowerShell counterpart of install.sh. It downloads the Windows archive, checks it against the
# release's SHA256SUMS, puts `keepster.exe` and `keepster-tray.exe` side by side in one lasting folder
# and runs `keepster install` from there, which registers the daemon and the tray at that path (ticket
# 27: an archive plus `keepster install` is the one install mechanism). Run as a file, its arguments go
# to `keepster install` (`install.ps1 --no-tray`) and its exit code is that command's; run through
# `iex`, it takes no arguments and leaves that code in $LASTEXITCODE rather than ending the session.
#
# Settings, all optional:
#   KEEPSTER_VERSION      the version to install, e.g. 0.1.0; by default, the release this script
#                         was published in
#   KEEPSTER_INSTALL_DIR  where the binaries go; by default %LOCALAPPDATA%\Programs\Keepster
#   KEEPSTER_BASE_URL     where the release's files are, as an https:// or file:// URL; by default
#                         its GitHub release
#
# PATH is left alone: nothing changes behind the user's back (ticket 27).

# One script block, so that run through `iex` nothing it defines is left in the caller's session.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    # Written by the release workflow when it publishes this script; empty in the repository.
    $ReleaseVersion = ''

    # Fetch `File` from the release into `Folder`.
    function Get-ReleaseFile([string] $Base, [string] $File, [string] $Folder) {
        $from = "$Base/$File"
        $to = Join-Path $Folder $File
        try {
            $uri = [Uri] $from
            if ($uri.IsFile) {
                Copy-Item -LiteralPath $uri.LocalPath -Destination $to
            } else {
                Invoke-WebRequest -UseBasicParsing -Uri $from -OutFile $to
            }
        } catch {
            throw "could not download ${from}: $($_.Exception.Message)"
        }
        $to
    }

    # `Value` as one argument on a Windows command line, quoted as the C runtime splits it: inside
    # quotes, a backslash run before a quote or the closing quote is doubled.
    function ConvertTo-Argument([string] $Value) {
        if ($Value -and $Value -notmatch '[\s"]') { return $Value }
        '"' + ($Value -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
    }

    # Install, and return `keepster install`'s exit code; a refusal is thrown.
    function Install-Keepster([string[]] $Arguments) {
        $version = if ($env:KEEPSTER_VERSION) { $env:KEEPSTER_VERSION } else { $ReleaseVersion }
        if (-not $version) {
            throw 'this copy names no release; set KEEPSTER_VERSION, e.g. $env:KEEPSTER_VERSION = ''0.1.0'''
        }

        # The release has one Windows build, for x64. A 32-bit PowerShell on 64-bit Windows reads the
        # machine's processor from PROCESSOR_ARCHITEW6432.
        $processor = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
        if ([Environment]::OSVersion.Platform -ne 'Win32NT' -or $processor -ne 'AMD64') {
            throw "no release build for $([Environment]::OSVersion.Platform) $processor; the Windows build is for x64 (AMD64)"
        }
        $name = "keepster-$version-x86_64-pc-windows-msvc"

        $folder = if ($env:KEEPSTER_INSTALL_DIR) { $env:KEEPSTER_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\Keepster' }
        $base = if ($env:KEEPSTER_BASE_URL) { $env:KEEPSTER_BASE_URL } else { "https://github.com/kravlab/keepster/releases/download/v$version" }

        # Windows PowerShell 5.1 on an older .NET setup offers no TLS 1.2, which GitHub requires.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

        $tmp = Join-Path ([IO.Path]::GetTempPath()) ("keepster-install-" + [Guid]::NewGuid())
        New-Item -ItemType Directory -Path $tmp | Out-Null
        try {
            $zip = Get-ReleaseFile $base "$name.zip" $tmp
            $sums = Get-ReleaseFile $base 'SHA256SUMS' $tmp

            # SHA256SUMS lines read `<hash>  <file>`, the file marked `*` when hashed as binary.
            $expected = $null
            foreach ($line in Get-Content -LiteralPath $sums) {
                $fields = -split $line
                if ($fields.Count -eq 2 -and $fields[1].TrimStart('*') -eq "$name.zip") { $expected = $fields[0] }
            }
            if (-not $expected) { throw "SHA256SUMS lists no $name.zip" }
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash -ne $expected) {
                throw "$name.zip does not match its checksum in SHA256SUMS"
            }
            Expand-Archive -LiteralPath $zip -DestinationPath $tmp

            # The full path, because `keepster install` registers the path it is run by.
            $dest = (New-Item -ItemType Directory -Force -Path $folder).FullName
            $keepster = Join-Path $dest 'keepster.exe'

            # Windows refuses to overwrite a running executable. So the daemon is stopped first, and
            # every tray running from this folder, an earlier upgrade's included: `keepster install`
            # starts both again from the new binaries (the tray unless `--no-tray` is given; its login
            # entry stays). A file still held anyway is renamed aside, which Windows allows, under a
            # name of its own, and removed on this run or the next.
            if (Test-Path -LiteralPath $keepster) {
                # A daemon never installed answers with an error, which changes nothing here.
                $ErrorActionPreference = 'Continue'
                & $keepster service stop 2>&1 | Out-Null
                $ErrorActionPreference = 'Stop'
            }
            $trays = @(Get-Process -Name 'keepster-tray' -ErrorAction SilentlyContinue |
                Where-Object { $_.Path -and [IO.Path]::GetDirectoryName($_.Path) -eq $dest })
            foreach ($tray in $trays) {
                Stop-Process -Id $tray.Id -Force -ErrorAction SilentlyContinue
                $tray.WaitForExit(10000) | Out-Null
            }
            foreach ($bin in 'keepster.exe', 'keepster-tray.exe') {
                $target = Join-Path $dest $bin
                Get-ChildItem -LiteralPath $dest -Filter "$bin.old*" |
                    Remove-Item -Force -ErrorAction SilentlyContinue
                $aside = "$target.old-$([Guid]::NewGuid().ToString('N'))"
                if (Test-Path -LiteralPath $target) { Move-Item -LiteralPath $target -Destination $aside }
                Copy-Item -LiteralPath (Join-Path $tmp "$name\$bin") -Destination $target
                Remove-Item -LiteralPath $aside -Force -ErrorAction SilentlyContinue
            }
        } finally {
            Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
        }

        # Started rather than called: a called command's output reaches the host through a pipe that
        # PowerShell reads to its end, and the tray `keepster install` starts inherits that pipe and
        # holds it open until logout. Started in this console, it writes there directly, and only the
        # command itself is waited for (`-Wait` would wait for the tray too). The handle is read first,
        # so that the exit code survives the process.
        $argumentList = @('install')
        foreach ($argument in $Arguments) { $argumentList += ConvertTo-Argument $argument }
        $process = Start-Process -FilePath $keepster -ArgumentList $argumentList -NoNewWindow -PassThru
        $null = $process.Handle
        $process.WaitForExit()
        $code = $process.ExitCode

        $onPath = @($env:PATH -split ';' | Where-Object { $_.TrimEnd('\') -eq $dest.TrimEnd('\') }).Count -gt 0
        if (-not $onPath) {
            Write-Host "keepster is at $keepster; $dest is not on PATH, so add it there to run plain ``keepster``"
        }
        $code
    }

    # Run through `iex`, `$args` would be whatever the caller's scope holds, so only a file run passes
    # arguments on.
    $arguments = if ($PSCommandPath) { @($args) } else { @() }
    try {
        $code = Install-Keepster $arguments
    } catch {
        [Console]::Error.WriteLine("keepster installer: $($_.Exception.Message)")
        $code = 1
    }
    # Run as a file, the exit code is the script's; run through `iex`, `exit` would end the caller's
    # session, so the code is left where a native command leaves its own.
    if ($PSCommandPath) { exit $code }
    $global:LASTEXITCODE = $code
} @args
