// Phase 2 E4 (ADR-016 G2): runs inside a headless VS Code with cortex-debug
// installed, on a project whose .vscode/ holds the committed templates
// (docs/templates/vscode) verbatim. scripts/vscode_test.sh starts VS Code
// with this file as --extensionTestsPath.
//
// It starts the template's debug configuration the way the Run and Debug
// view does (vscode.debug.startDebugging with the configuration's name), so
// VS Code runs the preLaunchTask from tasks.json, waits for its background
// problem matcher to report the run ready, then starts cortex-debug, which
// attaches its GDB to bb's proxy. Every Debug Adapter Protocol message is
// logged (BB_VSC_RESULT.dap.jsonl).
//
// BB_VSC_MODE:
//   attach  wait for the stop that the attach reports, read the threads,
//           the stack, every scope's variables (the core registers) and a
//           word of RAM at the stack pointer, then stop the session (the
//           adapter deletes its breakpoints and detaches) and let the run
//           finish;
//   break   the negative control: after the attach, set a function
//           breakpoint (BB_VSC_BREAK), continue, wait for the breakpoint
//           stop, remove it, then stop the session the same way.
//
// The result (BB_VSC_RESULT, JSON) records what happened; the shell script
// checks it and the run's outputs.
const vscode = require('vscode')
const fs = require('fs')

function withTimeout(p, ms, what) {
  return Promise.race([
    p,
    new Promise((_, rej) => setTimeout(() => rej(new Error(`timed out after ${ms} ms: ${what}`)), ms)),
  ])
}

exports.run = async function run() {
  const resultPath = process.env.BB_VSC_RESULT
  const mode = process.env.BB_VSC_MODE || 'attach'
  const configName = process.env.BB_VSC_CONFIG
  const t0 = Date.now()
  const result = { mode, config: configName, steps: [], ok: false }
  const dapLog = fs.openSync(resultPath + '.dap.jsonl', 'w')
  const step = (msg, extra) => {
    result.steps.push(Object.assign({ ms: Date.now() - t0, msg }, extra || {}))
    fs.writeFileSync(resultPath, JSON.stringify(result, null, 2) + '\n')
  }

  // Stopped events from the adapter, in order, as promises to wait for.
  const stops = []
  let stopWaiter = null
  const nextStop = () =>
    new Promise((res) => {
      if (stops.length) res(stops.shift())
      else stopWaiter = res
    })
  vscode.debug.registerDebugAdapterTrackerFactory('cortex-debug', {
    createDebugAdapterTracker() {
      return {
        onWillReceiveMessage: (m) => fs.writeSync(dapLog, JSON.stringify({ to: 'adapter', m }) + '\n'),
        onDidSendMessage: (m) => {
          fs.writeSync(dapLog, JSON.stringify({ from: 'adapter', m }) + '\n')
          if (m.type === 'event' && m.event === 'stopped') {
            if (stopWaiter) {
              const w = stopWaiter
              stopWaiter = null
              w(m.body)
            } else stops.push(m.body)
          }
        },
      }
    },
  })

  const taskEnded = new Promise((res) =>
    vscode.tasks.onDidEndTaskProcess((e) => res({ name: e.execution.task.name, exitCode: e.exitCode })),
  )
  const sessionEnded = new Promise((res) => vscode.debug.onDidTerminateDebugSession((s) => res(s.name)))
  const sessionStarted = new Promise((res) => vscode.debug.onDidStartDebugSession((s) => res(s)))

  try {
    const ext = vscode.extensions.getExtension('marus25.cortex-debug')
    if (!ext) throw new Error('cortex-debug is not installed')
    step(`cortex-debug ${ext.packageJSON.version}`)
    const folder = vscode.workspace.workspaceFolders[0]
    step(`workspace ${folder.uri.fsPath}`)

    const started = await withTimeout(
      vscode.debug.startDebugging(folder, configName),
      240000,
      'startDebugging (the preLaunchTask and the attach)',
    )
    if (!started) throw new Error('startDebugging returned false')
    const session = await withTimeout(sessionStarted, 10000, 'the debug session')
    step(`session started: ${session.name} (${session.type})`)

    const first = await withTimeout(nextStop(), 60000, 'the stop after the attach')
    step('stopped after the attach', { stop: first })
    const threads = await session.customRequest('threads')
    const threadId = first.threadId || threads.threads[0].id
    const stack = await session.customRequest('stackTrace', { threadId, startFrame: 0, levels: 5 })
    const top = stack.stackFrames[0]
    step('stack', { frames: stack.stackFrames.map((f) => f.name) })
    const scopes = await session.customRequest('scopes', { frameId: top.id })
    const reads = {}
    for (const sc of scopes.scopes) {
      if (!sc.variablesReference) continue
      const vars = await session.customRequest('variables', { variablesReference: sc.variablesReference })
      reads[sc.name] = vars.variables.map((v) => `${v.name}=${v.value}`)
    }
    result.scopes = reads
    step('scopes read', { scopes: Object.keys(reads) })
    const sp = await session.customRequest('evaluate', {
      expression: '*(unsigned int *)$sp',
      frameId: top.id,
      context: 'watch',
    })
    result.ram_at_sp = sp.result
    step(`RAM at $sp: ${sp.result}`)

    if (mode === 'break') {
      const fn = process.env.BB_VSC_BREAK
      const bp = new vscode.FunctionBreakpoint(fn)
      vscode.debug.addBreakpoints([bp])
      // Give the adapter the setFunctionBreakpoints request before continuing.
      await new Promise((r) => setTimeout(r, 2000))
      await session.customRequest('continue', { threadId })
      const hit = await withTimeout(nextStop(), 120000, `the breakpoint at ${fn}`)
      step('stopped at the breakpoint', { stop: hit })
      const st = await session.customRequest('stackTrace', { threadId: hit.threadId || threadId, levels: 1 })
      result.break_frame = st.stackFrames[0].name
      vscode.debug.removeBreakpoints([bp])
      await new Promise((r) => setTimeout(r, 2000))
    }

    await vscode.debug.stopDebugging(session)
    const ended = await withTimeout(sessionEnded, 30000, 'the end of the debug session')
    step(`session ended: ${ended}`)
    const task = await withTimeout(taskEnded, 240000, 'the end of the run (the task)')
    result.task = task
    step(`task ended: ${task.name}, exit code ${task.exitCode}`)
    result.ok = true
  } catch (e) {
    result.error = String(e && e.stack ? e.stack : e)
    step('error')
  } finally {
    fs.closeSync(dapLog)
    fs.writeFileSync(resultPath, JSON.stringify(result, null, 2) + '\n')
  }
  if (!result.ok) throw new Error(result.error)
}
