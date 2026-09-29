/** Exercises the Python host's actual OS boundary with trusted fixture code (11.5). */
import { spawn } from 'node:child_process'
import { cp, mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { pythonCommand } from '../server/sdk-hosts.ts'

const root = fileURLToPath(new URL('../../../', import.meta.url))
const home = join(root, 'apps/toolbox/.local')
await mkdir(home, { recursive: true })
const dir = await realpath(await mkdtemp(join(home, 'python-sandbox-')))
try {
  await cp(join(root, 'sdk/python/src/jevscript'), join(dir, '.toolbox/python/jevscript'), { recursive: true })
  const host = join(dir, 'probe.py')
  await writeFile(host, `import errno, os, socket, subprocess, sys
import jevscript
print('Python SDK import: ready', flush=True)
for fd in (3, 4):
    os.fstat(fd)
print('Inherited SDK channels: ready', flush=True)
for label, operation in (
    ('network', lambda: socket.create_connection(('127.0.0.1', 9), timeout=1)),
    ('process', lambda: subprocess.run([sys.executable, '-c', 'pass'], check=True)),
    ('outside file', lambda: open('/etc/passwd').read()),
):
    try:
        operation()
    except OSError as error:
        if label != 'outside file' and error.errno not in (errno.EPERM, errno.EACCES):
            raise
        print(label + ': denied', flush=True)
    else:
        raise RuntimeError(label + ' confinement failed')
print('Python sandbox fixture: passed', flush=True)
`)
  const command = await pythonCommand(dir, host)
  // Only this trusted, fixed fixture is logged. Generated hosts retain safe UI errors.
  console.log('Python sandbox fixture:', command.bin, 'on', process.platform)
  const child = spawn(command.bin, command.args, { cwd: dir, env: { PATH: process.env['PATH'], LANG: 'en_US.UTF-8', JEVS_TOOLBOX_EMBEDDED: '1' }, stdio: ['ignore', 'pipe', 'pipe', 'pipe', 'pipe'] })
  let bytes = 0
  const output = (chunk: Buffer) => {
    bytes += chunk.length
    if (bytes > 64 * 1024) child.kill('SIGKILL')
    else process.stdout.write(chunk)
  }
  child.stdout.on('data', output)
  child.stderr.on('data', output)
  const timeout = setTimeout(() => child.kill('SIGKILL'), 10_000)
  try {
    const code = await new Promise<number | null>((resolve, reject) => { child.once('error', reject); child.once('close', resolve) })
    if (code !== 0) throw new Error(`Python sandbox fixture exited with code ${code}.`)
  } finally { clearTimeout(timeout) }
} finally { await rm(dir, { recursive: true, force: true }) }
