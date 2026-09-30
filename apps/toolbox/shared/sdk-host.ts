/** Companion host examples using the actual section 11.2 SDK and toolbox context. */
export function sdkHost(fileName: string, language: 'typescript' | 'python' = 'typescript', task = 'main'): string {
  if (language === 'python') return `from jevscript import load
from toolbox_runtime import runtime, answer_pause, close_runtime


def main():
    program = load(${JSON.stringify(fileName)}, transport=runtime.transport)
    run = None
    try:
        run = program.task(${JSON.stringify(task)}).start(
            inputs=runtime.inputs, bind=runtime.bindings,
            record=runtime.recording, model=runtime.model, sample=runtime.sample,
        )
        for pause in run:
            if pause["kind"] == "done":
                print(pause.get("outputs", {}))
            elif pause["kind"] == "stopped":
                raise RuntimeError("The run stopped.")
            elif pause["kind"] == "error" and not pause.get("retryable", False):
                raise RuntimeError(pause.get("message", "The run failed."))
            elif pause["kind"] != "waiting":
                run.resume(answer_pause(pause))
    except Exception:
        if run is not None:
            run.abort()
        raise
    finally:
        program.close()
        close_runtime()


if __name__ == "__main__":
    main()
`
  return `import { load } from '@theaileverage/jevscript'
import { runtime, answerPause, closeRuntime } from './runtime.ts'

const program = await load(${JSON.stringify(fileName)}, { transport: runtime.transport })
let run
try {
  run = program.task(${JSON.stringify(task)}).start({
    inputs: runtime.inputs, bind: runtime.bindings,
    record: runtime.recording, model: runtime.model, sample: runtime.sample,
  })
  for await (const pause of run) {
    if (pause.kind === 'done') console.log(pause.outputs)
    else if (pause.kind === 'stopped') throw new Error('The run stopped.')
    else if (pause.kind === 'error' && !pause.retryable) throw new Error(pause.message)
    else if (pause.kind !== 'waiting') await run.resume(await answerPause(pause))
  }
} catch (error) {
  if (run) await run.abort().catch(() => {})
  throw error
} finally {
  await program.close()
  await closeRuntime()
}
`
}
