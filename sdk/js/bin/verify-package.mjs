import { existsSync, readFileSync, statSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../', import.meta.url))
const manifestPath = join(root, 'native', 'manifest.json')
if (!existsSync(manifestPath)) throw new Error('stage release binaries before npm pack')
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'))
if (manifest.version !== pkg.version) throw new Error('CLI and npm SDK versions differ')
const all = ['darwin-arm64', 'darwin-x64', 'linux-arm64-gnu', 'linux-x64-gnu', 'win32-x64']
const expected = process.env.JEVSCRIPT_PACK_TARGETS?.split(',') ?? all
for (const target of expected) {
  if (!all.includes(target)) throw new Error(`unsupported package target: ${target}`)
  const file = join(root, 'native', target, target === 'win32-x64' ? 'jevscript.exe' : 'jevscript')
  if (!existsSync(file) || !statSync(file).isFile()) throw new Error(`missing ${file}`)
  const digest = createHash('sha256').update(readFileSync(file)).digest('hex')
  if (manifest.targets[target]?.sha256 !== digest) throw new Error(`digest mismatch for ${target}`)
}
if (!existsSync(join(root, 'dist', 'native.js'))) throw new Error('build the JS SDK before packing')
if (!existsSync(join(root, 'examples', 'inbox_triage.jev'))) throw new Error('stage the shipped example')
if (!existsSync(join(root, 'examples', 'package_smoke.jev'))) throw new Error('stage the no-model smoke example')
const skill = join(root, 'skills', 'jevscript', 'SKILL.md')
if (!existsSync(skill) || createHash('sha256').update(readFileSync(skill)).digest('hex') !== manifest.skill_sha256) {
  throw new Error('staged Skill digest mismatch')
}
console.log(`verified Jevscript ${pkg.version}: ${expected.join(', ')}`)
