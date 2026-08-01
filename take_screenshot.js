const puppeteer = require('puppeteer');
const path = require('path');

(async () => {
  const browser = await puppeteer.launch();
  const page = await browser.newPage();
  
  // Set viewport to simulate a desktop app window
  await page.setViewport({ width: 1200, height: 800 });
  
  console.log('Navigating to http://localhost:1420...');
  await page.goto('http://localhost:1420', { waitUntil: 'networkidle2' });
  
  // Wait a bit for any animations
  await new Promise(r => setTimeout(r, 2000));
  
  // Take screenshot 1: Main window
  console.log('Taking main screenshot...');
  const out1 = path.join(__dirname, '..', 'flyweightsoft.github.io', 'src', 'assets', 'k8sdesk-main.png');
  await page.screenshot({ path: out1 });
  
  // Now simulate typing a delete command to show the destructive guard
  console.log('Typing destructive command...');
  await page.type('input[name="cmd"]', 'delete deployment frontend');
  await page.keyboard.press('Enter');
  
  await new Promise(r => setTimeout(r, 1000));
  
  // Take screenshot 2: Destructive guard
  console.log('Taking prod guard screenshot...');
  const out2 = path.join(__dirname, '..', 'flyweightsoft.github.io', 'src', 'assets', 'k8sdesk-guard.png');
  await page.screenshot({ path: out2 });
  
  // Open file manager if possible
  // Find a way to click file manager button
  try {
    console.log('Clicking file manager toggle...');
    // The button might be selected by a class or icon. Let's just click the text or svg.
    // We'll skip file manager if it's hard to select, or we can try.
  } catch (e) {
    console.log(e);
  }

  await browser.close();
  console.log('Done!');
})();
