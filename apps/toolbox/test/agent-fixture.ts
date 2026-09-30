/** Executable CLI stand-ins. They exercise process/stdin boundaries without reaching either provider. */
import { sdkHost } from '../shared/sdk-host.ts'
import { chmod, mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

export async function fakeAgents(home: string, source: string, language: 'typescript' | 'python' = 'typescript'): Promise<NodeJS.ProcessEnv> {
  const root = join(home, 'fake-agents')
  await mkdir(root, { recursive: true })
  const fileName = (/^program\s+(\w+)/m.exec(source)?.[1] ?? 'main') + '.jev'
  const companion = sdkHost(fileName, language)
  const script = `#!${process.execPath}
import { appendFileSync, existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
const root = dirname(fileURLToPath(import.meta.url))
const harness = process.argv[1].includes('codex') ? 'codex' : 'claude-code'
const args = process.argv.slice(2)
const emit = value => process.stdout.write(JSON.stringify(value) + '\\n')
const models = harness === 'codex' ? ['codex-first', 'codex-second'] : ['default', 'sonnet']
if (args[0] === 'auth') { process.stdout.write(JSON.stringify({loggedIn: !existsSync(join(root,'signed-out'))},null,2)); process.exit(0) }
const discovery = args[0] === 'app-server' || args.includes('--input-format')
let input = ''
process.stdin.on('data', chunk => {
  input += chunk
  if (!discovery) return
  let i
  while ((i = input.indexOf('\\n')) >= 0) {
    const request = JSON.parse(input.slice(0,i)); input = input.slice(i+1)
    if (harness === 'claude-code') emit({type:'control_response',response:{subtype:'success',response:{models:models.map(id=>({value:id,displayName:id,resolvedModel:'resolved-'+id}))}}})
    else if(request.method==='initialize') emit({id:request.id,result:{}})
    else if(request.method==='account/read') emit({id:request.id,result:{account:existsSync(join(root,'signed-out')) ? null : {type:'chatgpt'}}})
    else if(request.method==='model/list') emit({id:request.id,result:{data:models.map((id,i)=>({model:id,displayName:id,hidden:false,isDefault:i===0})),nextCursor:null}})
  }
})
process.stdin.on('end', () => {
  if (discovery) return
  const model = args[args.indexOf('--model')+1]
  appendFileSync(join(root,'calls.jsonl'),JSON.stringify({harness,model,args,input,cwd:process.cwd(),providerEnvironment:Object.keys(process.env).filter(key=>/^(OPENAI_|ANTHROPIC_|TYPESAFE_)/.test(key))})+'\\n')
  if (existsSync(join(root,'timeout'))) return setInterval(()=>{},1000)
  if (existsSync(join(root,'oversized'))) { process.stdout.write('x'.repeat(3*1024*1024)); return }
  if (existsSync(join(root,'failure'))) { process.stderr.write('PRIVATE_PROVIDER_SECRET'); process.exit(1) }
  const text = 'Executable '+harness+' '+model+' sorts each message into a lane.\\n\\n\\\`\\\`\\\`jev\\n'+${JSON.stringify(source)}+'\\\`\\\`\\\`'
  const conversation = JSON.parse(input.split('\\nConversation:\\n')[1])
  const savedFile = /The companion must load exactly "([^"]+)"/.exec(conversation.at(-1)?.content ?? '')?.[1]
  const host = savedFile ? ${JSON.stringify(companion)}.replace(${JSON.stringify(JSON.stringify(fileName))}, JSON.stringify(savedFile)) : ${JSON.stringify(companion)}
  const complete = text + '\\n\\n' + String.fromCharCode(96).repeat(3) + ${JSON.stringify(language + '\n')} + host + String.fromCharCode(96).repeat(3)
  if(harness==='claude-code') emit({type:'result',is_error:false,result:complete,modelUsage:{['resolved-'+model]:{}}})
  else { emit({type:'item.completed',item:{type:'agent_message',text:complete}});emit({type:'turn.completed'}); }
})
`
  for (const name of ['claude', 'codex']) {
    const path = join(root, `${name}.mjs`)
    await writeFile(path, script)
    await chmod(path, 0o755)
  }
  return { JEVS_TOOLBOX_CLAUDE_BIN: join(root, 'claude.mjs'), JEVS_TOOLBOX_CODEX_BIN: join(root, 'codex.mjs') }
}

export async function agentCalls(home: string): Promise<{ harness: string; model: string; args: string[]; input: string; cwd: string; providerEnvironment: string[] }[]> {
  try {
    const { readFile } = await import('node:fs/promises')
    return (await readFile(join(home, 'fake-agents/calls.jsonl'), 'utf8')).trim().split('\n').map(line => JSON.parse(line))
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return []
    throw error
  }
}
