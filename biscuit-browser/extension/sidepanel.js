// Biscuit Agent side panel: the chat UI. Sends tasks to the background agent
// loop and renders its transcript + inline approval prompts.
const api = globalThis.browser ?? globalThis.chrome

const logEl = document.getElementById('log')
const taskEl = document.getElementById('task')
const runEl = document.getElementById('run')
const stopEl = document.getElementById('stop')
const modeEl = document.getElementById('mode')
const statusEl = document.getElementById('status')

function bubble(cls, text) {
  const div = document.createElement('div')
  div.className = 'msg ' + cls
  div.textContent = text
  logEl.appendChild(div)
  logEl.scrollTop = logEl.scrollHeight
  return div
}

// Restore saved mode; persist on change.
api.storage.local.get(['mode']).then((d) => {
  if (d.mode) modeEl.value = d.mode
})
modeEl.addEventListener('change', () => api.storage.local.set({ mode: modeEl.value }))

document.getElementById('opts').addEventListener('click', (e) => {
  e.preventDefault()
  api.runtime.openOptionsPage()
})

function run() {
  const task = taskEl.value.trim()
  if (!task) return
  bubble('user', task)
  taskEl.value = ''
  api.runtime.sendMessage({ target: 'biscuit-bg', type: 'run', task })
}
runEl.addEventListener('click', run)
taskEl.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
    e.preventDefault()
    run()
  }
})
stopEl.addEventListener('click', () => api.runtime.sendMessage({ target: 'biscuit-bg', type: 'stop' }))

function approvalCard(id, proposal, gate) {
  const card = document.createElement('div')
  card.className = 'msg approval'
  const what = proposal.kind + (proposal.ref ? ' ' + proposal.ref : '') + (proposal.url ? ' ' + proposal.url : '')
  const riskCls = gate && gate.risk === 'high' ? 'risk-high' : ''
  const head = document.createElement('div')
  head.innerHTML = 'Approve action? <b></b> <span class="' + riskCls + '"></span>'
  head.querySelector('b').textContent = what
  head.querySelector('span').textContent = gate ? '[' + gate.risk + ']' : ''
  card.appendChild(head)
  if (proposal.text) {
    const t = document.createElement('div')
    t.style.cssText = 'opacity:.7;margin-top:4px'
    t.textContent = 'text: ' + String(proposal.text).slice(0, 120)
    card.appendChild(t)
  }
  const row = document.createElement('div')
  row.className = 'row'
  const yes = document.createElement('button')
  yes.className = 'primary'
  yes.textContent = 'Approve'
  const no = document.createElement('button')
  no.textContent = 'Deny'
  const respond = (approved) => {
    api.runtime.sendMessage({ target: 'biscuit-bg', type: 'approvalResponse', id, approved })
    yes.disabled = no.disabled = true
    head.appendChild(document.createTextNode(approved ? ' — approved' : ' — denied'))
  }
  yes.addEventListener('click', () => respond(true))
  no.addEventListener('click', () => respond(false))
  row.appendChild(yes)
  row.appendChild(no)
  card.appendChild(row)
  logEl.appendChild(card)
  logEl.scrollTop = logEl.scrollHeight
}

api.runtime.onMessage.addListener((msg) => {
  if (!msg || msg.target !== 'biscuit-panel') return
  if (msg.type === 'assistant') bubble('assistant', msg.text)
  else if (msg.type === 'log') bubble('system', msg.text)
  else if (msg.type === 'error') bubble('system', '⚠ ' + msg.message)
  else if (msg.type === 'status') statusEl.textContent = msg.status || ''
  else if (msg.type === 'approval') approvalCard(msg.id, msg.proposal, msg.gate)
})
