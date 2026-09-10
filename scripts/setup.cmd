@echo off
REM Windows blocks .ps1 files by default (ExecutionPolicy Restricted), so
REM running setup.ps1 directly fails with "running scripts is disabled on this
REM system" before a single line executes. This wrapper bypasses the policy for
REM this invocation only and changes no machine settings, so setup can be
REM started with `scripts\setup.cmd` or by double-clicking it.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup.ps1" %*
