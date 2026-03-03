import init, { BreenixTerminal } from '../pkg/breenix_web.js';

async function main() {
    // init() returns the raw wasm exports, including memory
    const wasm = await init();

    const WIDTH = 800;
    const HEIGHT = 600;

    const canvas = document.getElementById('display');
    canvas.width = WIDTH;
    canvas.height = HEIGHT;
    const ctx = canvas.getContext('2d');

    // BreenixTerminal now boots the kernel, populates the filesystem,
    // and writes the boot banner + initial prompt in Rust.
    const terminal = new BreenixTerminal(WIDTH, HEIGHT);
    const status = document.getElementById('status');
    status.textContent = `Terminal: ${terminal.cols()} cols \u00d7 ${terminal.rows()} rows`;

    // Cursor blink state
    let cursorVisible = true;
    let cursorTimer = 0;

    function render() {
        terminal.draw_cursor(cursorVisible);

        const ptr = terminal.buffer_ptr();
        const len = terminal.buffer_len();

        // Access wasm linear memory directly — zero copy
        const bytes = new Uint8ClampedArray(wasm.memory.buffer, ptr, len);
        const imageData = new ImageData(bytes, WIDTH, HEIGHT);
        ctx.putImageData(imageData, 0, 0);

        // Blink cursor every ~500ms (at 60 fps, toggle every 30 frames)
        cursorTimer++;
        if (cursorTimer >= 30) {
            cursorVisible = !cursorVisible;
            cursorTimer = 0;
        }

        requestAnimationFrame(render);
    }

    // Handle keyboard input
    canvas.tabIndex = 0;
    canvas.focus();

    canvas.addEventListener('keydown', (e) => {
        e.preventDefault();

        const ctrl = e.ctrlKey || e.metaKey;
        const shift = e.shiftKey;
        const key = e.key;

        // key_input now routes through the line discipline → shell → kernel.
        // Command output is rendered directly to the terminal framebuffer.
        terminal.key_input(key, ctrl, shift);
    });

    canvas.addEventListener('click', () => canvas.focus());

    // Start render loop
    requestAnimationFrame(render);
}

main();
