// Rasterize the vector app icon (resources/clipward-icon.svg) with Chromium at
// 1024px, then export every icon size from that render.
// Run through Electron via build-icons.sh:  electron scripts/icon/render.js <output-directory>
const fs = require('node:fs')
const path = require('node:path')
const { app, BrowserWindow } = require('electron')

const source = path.resolve(__dirname, '../../resources/clipward-icon.svg')
const outDir = process.argv[process.argv.length - 1]
if (!outDir || outDir.endsWith('render.js')) throw new Error('Usage: electron scripts/icon/render.js <output-directory>')

app.dock?.hide()
app.whenReady().then(async () => {
  const svg = fs.readFileSync(source, 'utf8')
  fs.mkdirSync(outDir, { recursive: true })
  const win = new BrowserWindow({
    width: 1024, height: 1024, show: false, transparent: true, frame: false,
    backgroundColor: '#00000000', useContentSize: true, webPreferences: { offscreen: true }
  })
  await win.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(`<html><body style="margin:0;background:transparent">${svg}</body></html>`)}`)
  const master = await win.webContents.capturePage({ x: 0, y: 0, width: 1024, height: 1024 })
  win.destroy()
  for (const size of [16, 24, 32, 48, 64, 128, 256, 512, 1024]) {
    fs.writeFileSync(path.join(outDir, `icon-${size}.png`), master.resize({ width: size, height: size, quality: 'best' }).toPNG())
  }
  console.log('Rendered app icon from resources/clipward-icon.svg')
  app.quit()
}).catch((error) => {
  console.error(error)
  app.exit(1)
})
