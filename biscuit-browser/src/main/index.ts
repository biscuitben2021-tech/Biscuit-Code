import { app, BrowserWindow, shell } from 'electron'
import { join } from 'node:path'
import { App } from './app'
import { SettingsStore } from './settings/store'

let mainWindow: BrowserWindow | null = null
let biscuit: App | null = null

function createWindow(): void {
  mainWindow = new BrowserWindow({
    width: 1440,
    height: 900,
    minWidth: 900,
    minHeight: 600,
    title: 'Biscuit Browser',
    backgroundColor: '#1b1b1f',
    show: false,
    webPreferences: {
      // The app UI renderer. Locked down: isolated context, no node, sandboxed.
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true
    }
  })

  mainWindow.on('ready-to-show', () => mainWindow?.show())

  // External links from the *app UI* (not browsed pages) open in the OS browser.
  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    void shell.openExternal(url)
    return { action: 'deny' }
  })

  const settings = new SettingsStore()
  biscuit = new App(mainWindow, settings)

  // electron-vite injects ELECTRON_RENDERER_URL in dev; load the file in prod.
  if (process.env.ELECTRON_RENDERER_URL) {
    void mainWindow.loadURL(process.env.ELECTRON_RENDERER_URL)
  } else {
    void mainWindow.loadFile(join(__dirname, '../renderer/index.html'))
  }

  mainWindow.on('closed', () => {
    biscuit?.destroy()
    biscuit = null
    mainWindow = null
  })
}

app.whenReady().then(() => {
  createWindow()
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit()
})
