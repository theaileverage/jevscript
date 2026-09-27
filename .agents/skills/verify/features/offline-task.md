# Offline task

## Sub-features

Load and execute a no-model task through the JS and Python SDKs.

## How to get to it (user POV)

Import `jevscript` from an installed package, load a `.jev` program, and run its `main` task.

## Driving it with the package smoke

Run the full package smoke in `docs/package-release.md`. It loads the packaged `package_smoke.jev` through each SDK and requires a `done` pause, then damages each installed native binary and requires an integrity failure.

## Gotchas

The task uses no live Jev model or adapter. Label this local E2E, not a live service result.
