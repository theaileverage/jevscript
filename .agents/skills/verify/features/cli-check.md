# CLI check

## Sub-features

Validate a Jevscript program and print diagnostics to the host.

## How to get to it (user POV)

Run `jevscript check path/to/program.jev` after installing a package.

## Driving it with the package smoke

The installed-package smoke runs each installed command against its packaged `inbox_triage.jev` fixture. Observe exit 0 and no error diagnostic; a package-local example path proves the installation carries its inputs.

## Gotchas

Checking a source-tree example with a source-tree binary proves only a local CLI, not package installation.
