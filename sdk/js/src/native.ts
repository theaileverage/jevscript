/** Resolve the release's packaged CLI (spec section 11.5). */
import { createHash } from 'node:crypto'
import { accessSync, constants, existsSync, readFileSync, statSync } from 'node:fs'
import { arch, platform, report } from 'node:process'
import { fileURLToPath } from 'node:url'

interface BinaryManifest {
  version: string
  targets: Record<string, { sha256: string }>
}

/** Match only release targets that have been built and tested. */
export function nativeTarget(): string {
  if (platform === 'darwin' && (arch === 'arm64' || arch === 'x64')) return `darwin-${arch}`
  if (platform === 'win32' && arch === 'x64') return 'win32-x64'
  if (platform === 'linux' && (arch === 'arm64' || arch === 'x64')) {
    const diagnostic = report.getReport() as { header?: { glibcVersionRuntime?: string } }
    if (diagnostic.header?.glibcVersionRuntime) return `linux-${arch}-gnu`
    throw new Error('Jevscript does not yet ship a Linux musl/Alpine CLI')
  }
  throw new Error(`Jevscript has no CLI for ${platform}/${arch}`)
}

/** Verify the packaged binary before executing it, including SDK subprocesses. */
export function packagedBinary(): string {
  const target = nativeTarget()
  const root = new URL('../', import.meta.url)
  let manifest: BinaryManifest
  try {
    manifest = JSON.parse(readFileSync(new URL('native/manifest.json', root), 'utf8')) as BinaryManifest
  } catch {
    throw new Error('Jevscript CLI manifest is missing or invalid; reinstall the complete package')
  }
  const pkg = JSON.parse(readFileSync(new URL('package.json', root), 'utf8')) as { version: string }
  if (manifest.version !== pkg.version) throw new Error('Jevscript CLI version does not match the SDK')
  const expected = manifest.targets[target]?.sha256
  if (!expected) throw new Error(`Jevscript CLI for ${target} is missing; reinstall the complete package`)
  const path = fileURLToPath(new URL(`native/${target}/jevscript${platform === 'win32' ? '.exe' : ''}`, root))
  if (!existsSync(path) || !statSync(path).isFile()) {
    throw new Error(`Jevscript CLI for ${target} is missing; reinstall the complete package`)
  }
  if (platform !== 'win32') {
    try { accessSync(path, constants.X_OK) } catch {
      throw new Error(`Jevscript CLI for ${target} is not executable; reinstall the package`)
    }
  }
  const digest = createHash('sha256').update(readFileSync(path)).digest('hex')
  if (digest !== expected) throw new Error(`Jevscript CLI for ${target} failed integrity check; reinstall the package`)
  return path
}

/** Explicit caller choice wins; ordinary installations use their matching CLI. */
export function resolveBinary(explicit?: string): string {
  return explicit ?? process.env['JEVSCRIPT_BIN'] ?? packagedBinary()
}
