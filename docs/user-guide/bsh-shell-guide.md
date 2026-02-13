# Breenish (bsh) Shell Guide

Breenish is the ECMAScript-powered shell for Breenix. It combines a full JavaScript engine with POSIX process execution, giving you real data structures, closures, async/await, and `try/catch` instead of bash's ad-hoc scripting.

---

## Quick Start

```
./run.sh --clean           # ARM64 (default) - full rebuild
./run.sh --clean --x86     # x86_64 - full rebuild
```

Once Breenix boots, you'll see:

```
bsh /bin>
```

Type JavaScript or shell commands directly.

---

## Running Commands

### Bare Commands (Auto-Exec)

Type a command name and bsh auto-wraps it in `exec()`:

```
bsh /> ls /bin
bsh /> cat /etc/bshrc
bsh /> echo hello world
```

Behind the scenes, `ls /bin` becomes `exec("ls", "/bin")`.

### Explicit Process Execution

```javascript
let r = exec("ls", "-la", "/bin");
// r = { exitCode: 0, stdout: "...", stderr: "", pid: 42 }

print(r.stdout);
print(r.exitCode);
```

### Pipelines

```javascript
let r = pipe("cat /etc/bshrc", "grep let", "wc -l");
print(r.stdout);
```

Each argument is a space-separated command string. Pipe connects stdout of each stage to stdin of the next.

### Command Resolution

```javascript
which("ls")      // "/bin/ls"
which("nope")    // null
```

### Background and Parallel Execution

```javascript
// Don't await = runs in background (when async is available)
let server = exec("telnetd");

// Parallel
let [a, b] = await Promise.all([
    exec("echo", "one"),
    exec("echo", "two")
]);
```

---

## File Operations

```javascript
// Read a file
let contents = readFile("/etc/bshrc");
print(contents);

// Write a file
writeFile("/tmp/hello.txt", "Hello from bsh!\n");

// Glob (wildcard expansion)
let files = glob("/bin/*");
print(files.length + " files in /bin");

let rust = glob("/src/*.rs");
for (let f of rust) {
    print(f);
}
```

---

## Environment Variables

```javascript
// Get all env vars as an object
let all = env();
print(JSON.stringify(all));

// Get one
let path = env("PATH");
print(path);

// Set one
env("MY_VAR", "hello");
print(env("MY_VAR"));   // "hello"
```

---

## Navigation

```javascript
cd("/bin");      // Change directory
pwd()            // Print working directory: "/bin"
cd("/");         // Back to root

// The prompt shows your current directory:
// bsh /bin>
```

---

## JavaScript Language Features

bsh runs full ECMAScript. Everything below works at the REPL or in scripts.

### Variables and Types

```javascript
let name = "Breenix";
const version = 1;
let items = [1, 2, 3];
let config = { debug: true, port: 8080 };
```

### Template Literals

```javascript
let user = "root";
print(`Hello ${user}, you have ${items.length} items`);
```

### Functions and Closures

```javascript
function greet(name) {
    return `Hello, ${name}!`;
}
print(greet("world"));

// Arrow functions
let double = (x) => x * 2;
let nums = [1, 2, 3].map(x => x * 10);
print(nums);  // [10, 20, 30]

// Closures
function counter() {
    let n = 0;
    return () => { n++; return n; };
}
let c = counter();
print(c());  // 1
print(c());  // 2
```

### Control Flow

```javascript
// if/else
if (env("DEBUG")) {
    print("debug mode");
} else {
    print("production");
}

// Ternary
let mode = env("DEBUG") ? "debug" : "prod";

// for loops
for (let i = 0; i < 5; i++) {
    print(i);
}

// for...of (arrays)
for (let file of glob("/bin/*")) {
    print(file);
}

// for...in (object keys)
let obj = { a: 1, b: 2, c: 3 };
for (let key in obj) {
    print(`${key} = ${obj[key]}`);
}

// while / do-while
let x = 10;
while (x > 0) { x--; }
do { x++; } while (x < 5);

// switch
switch (env("SHELL")) {
    case "/bin/bsh":
        print("breenish!");
        break;
    default:
        print("other shell");
}
```

### Error Handling

```javascript
try {
    let r = exec("nonexistent-command");
    if (r.exitCode !== 0) {
        throw new Error("command failed: " + r.exitCode);
    }
} catch (e) {
    console.error("Caught:", e.message);
} finally {
    print("cleanup done");
}
```

### Destructuring

```javascript
// Object destructuring
let { stdout, exitCode } = exec("uname");
print(stdout);

// Array destructuring
let [first, second] = ["hello", "world"];

// In function params
function show({ stdout, exitCode }) {
    print(`exit=${exitCode}: ${stdout}`);
}
show(exec("echo", "hi"));
```

### Spread and Nullish Coalescing

```javascript
// Spread in function calls
let args = ["-la", "/bin"];
exec("ls", ...args);

// Nullish coalescing
let port = env("PORT") ?? "8080";
```

### Async/Await

```javascript
async function deploy() {
    let build = await exec("make");
    if (build.exitCode !== 0) {
        throw new Error("build failed");
    }
    let test = await exec("make", "test");
    return test.exitCode;
}
```

---

## Built-in Objects

### console

```javascript
console.log("info message");           // stdout
console.error("error message");        // stderr
console.warn("same as error");         // stderr
console.info("same as log");           // stdout
console.log("multiple", "args", 42);   // space-separated
```

### JSON

```javascript
let data = JSON.parse('{"name": "bsh", "version": 1}');
print(data.name);    // "bsh"
print(data.version); // 1

let s = JSON.stringify({ a: [1, 2, 3], b: true });
print(s);  // '{"a":[1,2,3],"b":true}'
```

### Math

```javascript
Math.floor(3.7)    // 3
Math.ceil(3.2)     // 4
Math.round(3.5)    // 4
Math.abs(-5)       // 5
Math.min(1, 2)     // 1
Math.max(1, 2)     // 2
Math.pow(2, 10)    // 1024
Math.sqrt(144)     // 12
Math.random()      // 0.0 to 1.0
Math.PI            // 3.14159...
Math.E             // 2.71828...
Math.log(Math.E)   // 1
Math.trunc(3.9)    // 3
```

### Number

```javascript
Number.isInteger(5)       // true
Number.isFinite(Infinity) // false
Number.isNaN(NaN)         // true
Number.parseInt("42abc")  // 42
Number.parseFloat("3.14") // 3.14

// Also available as globals
parseInt("10")
parseFloat("2.5")
```

### Map

```javascript
let m = Map();
m.set("name", "bsh");
m.set("version", 1);
print(m.get("name"));     // "bsh"
print(m.has("name"));     // true
print(m.size);             // 2
print(m.keys());           // ["name", "version"]
print(m.values());         // ["bsh", 1]
m.delete("version");
m.clear();
```

### Set

```javascript
let s = Set();
s.add("apple");
s.add("banana");
s.add("apple");            // duplicate ignored
print(s.size);             // 2
print(s.has("apple"));     // true
print(s.values());         // ["apple", "banana"]
s.delete("apple");
```

### Promise

```javascript
Promise.resolve(42)               // fulfilled with 42
Promise.reject("oops")            // rejected
Promise.all([p1, p2, p3])         // wait for all
Promise.race([p1, p2, p3])        // first to settle
Promise.allSettled([p1, p2, p3])   // all results with status
```

---

## Array Methods

```javascript
let a = [3, 1, 4, 1, 5];

// Basics
a.push(9);               // [3,1,4,1,5,9], returns new length
a.pop();                  // returns 9, array is [3,1,4,1,5]
a.indexOf(4);             // 2
a.includes(7);            // false
a.join(" - ");            // "3 - 1 - 4 - 1 - 5"
a.slice(1, 3);            // [1, 4] (new array)
a.concat([6, 7]);         // [3,1,4,1,5,6,7] (new array)
a.reverse();              // reverses in place

// Higher-order
a.map(x => x * 2);                    // [6,2,8,2,10]
a.filter(x => x > 2);                 // [3,4,5]
a.reduce((sum, x) => sum + x, 0);     // 14
a.forEach(x => print(x));             // prints each
a.find(x => x > 3);                   // 4
a.some(x => x > 4);                   // true
a.every(x => x > 0);                  // true
a.flat();                              // flatten nested arrays

// Length
print(a.length);
```

---

## String Methods

```javascript
let s = "Hello, World!";

s.length                    // 13
s.indexOf("World")          // 7
s.includes("World")         // true
s.startsWith("Hello")       // true
s.endsWith("!")             // true
s.toUpperCase()             // "HELLO, WORLD!"
s.toLowerCase()             // "hello, world!"
s.trim()                    // removes whitespace
s.replace("World", "Breenix")  // "Hello, Breenix!"
s.slice(0, 5)               // "Hello"
s.charAt(0)                 // "H"
s.split(", ")               // ["Hello", "World!"]
```

---

## REPL Features

### Line Editing

| Key | Action |
|-----|--------|
| Left/Right | Move cursor |
| Home / Ctrl+A | Start of line |
| End / Ctrl+E | End of line |
| Backspace | Delete before cursor |
| Delete | Delete at cursor |
| Ctrl+K | Delete to end of line |
| Ctrl+U | Delete to start of line |
| Ctrl+W | Delete previous word |
| Ctrl+C | Cancel current line |
| Ctrl+D | Exit shell (empty line) |
| Up/Down | Command history |
| Tab | Auto-complete |

### Tab Completion

- **Command position**: Searches PATH for matching executables
- **Argument position**: Searches current or specified directory for files
- **Single match**: Completes fully (adds space for commands, `/` for directories)
- **Multiple matches**: Shows all options and completes common prefix

### Expression Results

The REPL auto-prints non-undefined results (like Node.js):

```
bsh /> 1 + 2
3
bsh /> "hello".toUpperCase()
'HELLO'
bsh /> [1,2,3].map(x => x * 2)
[2, 4, 6]
```

### Startup Script

bsh loads `/etc/bshrc` on startup. Put your aliases, environment setup, and helpers there:

```javascript
// /etc/bshrc
env("PATH", "/bin:/usr/bin");
env("EDITOR", "vi");

function ll() { return exec("ls", "-la"); }
```

### Script Execution

```bash
bsh script.js         # Run a file
bsh -e 'print(1+1)'   # Evaluate a string
source("helpers.js")   # Load a file in the REPL
```

---

## Recipes

### List files with details

```javascript
let { stdout } = exec("ls", "-la", "/bin");
let lines = stdout.split("\n");
for (let line of lines) {
    if (line.includes(".elf")) {
        print(line);
    }
}
```

### Simple build script

```javascript
function build(target) {
    let r = exec("make", target);
    if (r.exitCode !== 0) {
        console.error(`Build failed for ${target}:`);
        console.error(r.stderr);
        return false;
    }
    print(`Built ${target} successfully`);
    return true;
}

if (build("kernel") && build("userspace")) {
    print("All builds passed!");
}
```

### Process output as data

```javascript
let { stdout } = exec("ls", "/bin");
let files = stdout.trim().split("\n");
let elfs = files.filter(f => f.endsWith(".elf"));
print(`${elfs.length} ELF binaries in /bin`);
```

### Environment-aware configuration

```javascript
let debug = env("DEBUG") ?? "0";
let port = parseInt(env("PORT") ?? "8080");
let config = {
    debug: debug !== "0",
    port: port,
    host: env("HOST") ?? "0.0.0.0"
};
print(JSON.stringify(config));
```

### Glob and iterate

```javascript
for (let f of glob("/bin/*.elf")) {
    let name = f.split("/").pop().replace(".elf", "");
    print(`  ${name}`);
}
```
