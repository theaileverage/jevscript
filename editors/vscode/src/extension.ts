// The VS Code client for `jevscript lsp`. Everything a `.jev` file gets —
// diagnostics, highlighting, hover, navigation, outline, folding and
// completion — comes from the language server; this file only starts it.

import * as vscode from 'vscode'
import {
  LanguageClient,
  type LanguageClientOptions,
  type ServerOptions,
  TransportKind,
} from 'vscode-languageclient/node'

import { type Settings, initializationOptions, serverCommand } from './server'

let client: LanguageClient | undefined

function settings(): Settings {
  const config = vscode.workspace.getConfiguration('jevscript')
  return {
    serverPath: config.get<string>('server.path', 'jevscript'),
    modulePaths: config.get<string[]>('modulePaths', []),
    errorReference: config.get<string>('errorReference', ''),
  }
}

async function start(context: vscode.ExtensionContext): Promise<void> {
  const current = settings()
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath
  const { command, args } = serverCommand(current, folder)
  const serverOptions: ServerOptions = {
    run: { command, args, transport: TransportKind.stdio },
    debug: { command, args, transport: TransportKind.stdio },
  }
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'jevscript' },
      { scheme: 'untitled', language: 'jevscript' },
    ],
    initializationOptions: initializationOptions(current),
    synchronize: {
      // A library saved outside the editor changes what its importers compile to.
      fileEvents: vscode.workspace.createFileSystemWatcher('**/*.jev'),
    },
  }
  client = new LanguageClient('jevscript', 'Jevscript', serverOptions, clientOptions)
  context.subscriptions.push(client)
  try {
    await client.start()
  } catch (error) {
    void vscode.window.showErrorMessage(
      `Could not start \`${command} lsp\`: ${String(error)}. Install jevscript or set jevscript.server.path.`,
    )
  }
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  context.subscriptions.push(
    vscode.commands.registerCommand('jevscript.restartServer', async () => {
      await client?.stop()
      client = undefined
      await start(context)
    }),
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration('jevscript')) {
        await vscode.commands.executeCommand('jevscript.restartServer')
      }
    }),
  )
  await start(context)
}

export async function deactivate(): Promise<void> {
  await client?.stop()
}
