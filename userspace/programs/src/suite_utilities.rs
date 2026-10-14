//! Shell & POSIX utilities: the shell command language and the utilities a script runs, as
//! POSIX.1-2024's Shell and Utilities volume (XCU) specifies them in its Shell Command
//! Language chapter and each utility's page.
//!
//! Every case runs /bin/sh, or a utility found by its POSIX name through PATH, with the
//! environment PATH=/bin:/usr/bin and HOME=/tmp, umask 022, every signal at its default
//! action and none blocked, fixed standard input and fixed files in a scratch directory of
//! its own under /tmp, and compares the exit status, the exact standard output and whether
//! anything was written to standard error with what POSIX specifies. Where POSIX leaves
//! output layout to the implementation (padding, dates, sizes), the case reads the fields it
//! needs with the shell rather than comparing the layout. The expected values were checked
//! against dash and GNU coreutils on Linux, used only as a behaviour oracle. Before it runs
//! anything, a case checks that each utility it uses is on PATH and fails naming those that
//! are not.
//!
//! The scripts category runs whole scripts written to files with `sh script.sh`, the way
//! they run elsewhere.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit. The
//! program it starts gets until 1.5 seconds before that limit; one still running then is
//! killed and the case fails saying it did not finish.
use libbreenix::fs;
use libbreenix::io::{self, fcntl_cmd, poll_events, status_flags, PollFd};
use libbreenix::process;
use libbreenix::signal;
use libbreenix::suite::{case, case_ms_left, category, fail, suite, CaseError, CaseResult, Suite};
use libbreenix::syscall::{nr, raw};
use libbreenix::time;
use libbreenix::types::Fd;
use libbreenix::error::Error;
use libbreenix::Errno;
use std::ffi::CString;
use std::os::unix::fs::PermissionsExt;

/// The shell every `sh` case runs.
const SH: &str = "/bin/sh";
/// The search path, also the whole environment with HOME.
const PATH: &str = "/bin:/usr/bin";
const ENV: &[&str] = &["PATH=/bin:/usr/bin", "HOME=/tmp"];
/// How much of the case's limit is kept back for killing and reporting a program that hangs.
const RESERVE_MS: u64 = 1500;
/// An expected exit status of "any non-zero value", where POSIX specifies only that.
const NONZERO: i32 = -1;

/// Whether the case expects the program to write to standard error.
#[derive(Clone, Copy, PartialEq)]
enum Stderr {
    Empty,
    Written,
}
use Stderr::{Empty, Written};

static SUITE: Suite = suite("utilities", "Shell & POSIX utilities", &[
    category("shell", "POSIX sh language", &[
        case("quote-single", "Single quotes keep every character literally, including $ and double quotes", shell_quote_single),
        case("quote-double", "Double quotes expand parameters but keep \\$, \\\", \\\\ and single quotes literal", shell_quote_double),
        case("param-default", "${x:-word} substitutes for unset or empty, ${x-word} only for unset", shell_param_default),
        case("param-length", "${#x} expands to the length of x in characters", shell_param_length),
        case("param-trim", "${x#p}, ${x##p}, ${x%p} and ${x%%p} remove the shortest and longest matching prefix and suffix", shell_param_trim),
        case("cmdsub", "$(...) and backquotes substitute a command's output with trailing newlines removed", shell_cmdsub),
        case("arith", "$((...)) evaluates C integer arithmetic with precedence, %, shifts, and hex and octal constants", shell_arith),
        case("field-split-ifs", "Field splitting on a non-whitespace IFS keeps empty fields between delimiters", shell_field_split_ifs),
        case("glob", "Pathname expansion matches *, ? and [!...] against the directory, sorted", shell_glob),
        case("expansion-order", "Pathname expansion applies to parameter expansion results, which are not expanded again", shell_expansion_order),
        case("heredoc", "A here-document with an unquoted delimiter expands parameters and commands and honours \\$", shell_heredoc),
        case("noclobber", "With set -C, > refuses to overwrite an existing file and >| overwrites it", shell_noclobber),
        case("fd-dup", "2>&1 sends standard error to where standard output goes", shell_fd_dup),
        case("pipeline", "A pipeline connects stdout to stdin and its status is the last command's", shell_pipeline),
        case("pipeline-bang", "! inverts a pipeline's exit status", shell_pipeline_bang),
        case("and-or", "&& runs on success, || on failure, with equal precedence left to right", shell_and_or),
        case("if-elif", "if, elif and else run the branch of the first true condition", shell_if_elif),
        case("case", "case matches patterns with *, |, bracket classes and the empty string, in order", shell_case),
        case("for-loop", "for iterates over a word list, and over \"$@\" when in is left out", shell_for_loop),
        case("while-until", "while loops while its condition succeeds and until while it fails", shell_while_until),
        case("function-positional", "A function gets its own positional parameters and the caller's are restored", shell_function_positional),
        case("function-return", "return n sets a function's status, and without return it is the last command's", shell_function_return),
        case("special-params", "$# counts, \"$@\" keeps each parameter and \"$*\" joins them with the first IFS character", shell_special_params),
        case("special-pid", "$$ is the shell's PID in subshells too, and $! names the last background job for wait", shell_special_pid),
        case("set-e", "set -e exits on a failing command but not in an if condition, an || list or after !", shell_set_e),
        case("set-u", "set -u makes expanding an unset parameter an error that exits, but ${x-word} is allowed", shell_set_u),
        case("set-f", "set -f disables pathname expansion and set +f restores it", shell_set_f),
        case("export", "export passes a variable to child processes, and an assignment prefix only to that command", shell_export),
        case("readonly", "Assigning to a readonly variable is an error that exits the (sub)shell", shell_readonly),
        case("unset", "unset removes a variable and unset -f a function", shell_unset),
        case("shift", "shift n drops the first n positional parameters", shell_shift),
        case("eval", "eval joins its arguments and runs them as a command", shell_eval),
        case("exec", "exec with a command replaces the shell, so later commands never run", shell_exec),
        case("trap-exit", "A trap on EXIT runs when the shell exits and keeps the exit status", shell_trap_exit),
        case("trap-signal", "A trap on a signal runs its action when the shell receives the signal", shell_trap_signal),
        case("exit", "exit n ends the shell with status n, and a subshell's exit ends only the subshell", shell_exit),
        case("dot", ". runs a file in the current shell, and return in it ends the file with a status", shell_dot),
        case("cd-pwd", "cd changes the directory and PWD, pwd agrees, and cd to a missing directory fails", shell_cd_pwd),
        case("signal-status", "A command killed by a signal has exit status 128 plus the signal number", shell_signal_status),
        case("subshell", "( ) runs in a subshell whose changes do not persist, while { } runs in the current shell", shell_subshell),
    ]),
    category("file-utils", "file utilities", &[
        case("ls", "ls lists a directory's names sorted, one per line when stdout is not a terminal, without dot files", file_ls),
        case("ls-a", "ls -a includes . , .. and names starting with a period", file_ls_a),
        case("ls-d", "ls -d lists a directory operand itself rather than its contents", file_ls_d),
        case("ls-l", "ls -l shows the mode string, link count, size and name of a file", file_ls_l),
        case("ls-l-dir", "ls -ld shows a directory's type and permission bits", file_ls_l_dir),
        case("cp", "cp copies a file's contents to a new file", file_cp),
        case("cp-r", "cp -R copies a directory hierarchy", file_cp_r),
        case("cp-dir-needs-r", "cp of a directory without -R writes a diagnostic and exits non-zero", file_cp_dir_needs_r),
        case("cp-p", "cp -p preserves the file's modification time and permission bits", file_cp_p),
        case("mv-across", "mv moves a file into another directory", file_mv_across),
        case("mv-dir", "mv renames a directory together with its contents", file_mv_dir),
        case("rm-r", "rm -r removes a directory hierarchy", file_rm_r),
        case("rm-missing", "rm of a missing file writes a diagnostic and exits non-zero", file_rm_missing),
        case("rm-f", "rm -f of a missing file is silent and exits 0", file_rm_f),
        case("mkdir-p", "mkdir -p creates missing parents and accepts directories that exist", file_mkdir_p),
        case("rmdir", "rmdir removes an empty directory", file_rmdir),
        case("rmdir-nonempty", "rmdir of a directory that is not empty fails and leaves it", file_rmdir_nonempty),
        case("ln", "ln makes a hard link that shares the file's contents and raises its link count", file_ln),
        case("ln-s", "ln -s makes a symbolic link that reads through to its target", file_ln_s),
        case("chmod-octal", "chmod with an octal mode sets the permission bits", file_chmod_octal),
        case("chmod-symbolic", "chmod with a symbolic mode adds, removes and sets bits per class", file_chmod_symbolic),
        case("touch-create", "touch creates an empty file that does not exist", file_touch_create),
        case("touch-t", "touch -t sets a file's modification time, which ls -l shows", file_touch_t),
        case("test-nt", "A file touched with an older time is older than a new file to test -nt and -ot", file_test_nt),
        case("find-name", "find -name matches file names against a pattern throughout the tree", file_find_name),
        case("find-type", "find -type d selects directories, including the starting point", file_find_type),
        case("find-mtime", "find -mtime +n selects files modified more than n days ago", file_find_mtime),
        case("find-exec", "find -exec runs a command for each file with {} replaced by its path", file_find_exec),
        case("find-prune", "find -prune keeps find from descending into a directory", file_find_prune),
        case("du", "du writes one line per directory in the hierarchy, ending with the operand", file_du),
        case("df", "df -k -P writes the POSIX header and a line for the file system holding the operand", file_df),
        case("test-file-ops", "test's -e, -f, -d, -s, -r, -w, -x and -h operators describe files", file_test_file_ops),
        case("bracket-utility", "[ found through PATH evaluates its expression and requires the closing ]", file_bracket_utility),
        case("basename", "basename strips the directory and a suffix, and handles / and trailing slashes", file_basename),
        case("dirname", "dirname strips the last component, giving . for a bare name and / for /", file_dirname),
        case("readlink", "readlink writes the contents of a symbolic link", file_readlink),
        case("realpath", "realpath resolves . , .. and symbolic links to an absolute path", file_realpath),
    ]),
    category("text-utils", "text utilities", &[
        case("cat", "cat concatenates files and - for standard input in order", text_cat),
        case("head-n", "head -n 2 writes the first two lines", text_head_n),
        case("head-c", "head -c 5 writes the first five bytes", text_head_c),
        case("tail-n", "tail -n 2 writes the last two lines", text_tail_n),
        case("wc", "wc counts lines, words and bytes, in that order", text_wc),
        case("sort", "sort orders lines by byte value in the POSIX locale", text_sort),
        case("sort-n", "sort -n orders by numeric value, including negative numbers", text_sort_n),
        case("sort-r", "sort -r reverses the order", text_sort_r),
        case("sort-k", "sort -k 2,2n sorts numerically on the second field", text_sort_k),
        case("sort-u", "sort -u writes one line of each run of equal lines", text_sort_u),
        case("uniq-c", "uniq -c precedes each line with the number of times it occurred", text_uniq_c),
        case("cut-f", "cut -d : -f 1,3 selects fields by delimiter", text_cut_f),
        case("cut-c", "cut -c 2-4 selects a range of characters", text_cut_c),
        case("tr-range", "tr a-z A-Z translates a range of characters", text_tr_range),
        case("tr-d", "tr -d deletes the characters in a set", text_tr_d),
        case("tr-s", "tr -s squeezes repeated characters to one", text_tr_s),
        case("sed-s", "sed s/// replaces the first match, and with g every match", text_sed_s),
        case("sed-n-p", "sed -n prints only what p prints, with $ addressing the last line", text_sed_n_p),
        case("sed-range", "sed 2,3d deletes a range of lines", text_sed_range),
        case("awk-fields", "awk -F splits fields at the separator and sets NF", text_awk_fields),
        case("awk-pattern", "awk runs an action only for records its pattern selects", text_awk_pattern),
        case("awk-begin-end", "awk runs BEGIN before input and END after it, with NR counting records", text_awk_begin_end),
        case("awk-printf", "awk printf formats %5.2f, %-3s, %d and %x", text_awk_printf),
        case("grep-e", "grep -E matches extended regular expressions with alternation and grouping", text_grep_e),
        case("grep-v", "grep -v writes the lines that do not match", text_grep_v),
        case("grep-c", "grep -c writes the number of matching lines", text_grep_c),
        case("grep-l", "grep -l writes the names of the files that contain a match", text_grep_l),
        case("grep-n", "grep -n precedes each matching line with its line number", text_grep_n),
        case("grep-i", "grep -i matches without regard to case", text_grep_i),
        case("grep-status", "grep exits 1 when nothing matches and more than 1 for a missing file", text_grep_status),
        case("diff-same", "diff of identical files writes nothing and exits 0", text_diff_same),
        case("diff-differ", "diff writes the default change format and exits 1 when files differ", text_diff_differ),
        case("cmp", "cmp names the first differing byte and line and exits 1", text_cmp),
        case("comm", "comm writes lines unique to each file and common lines in three tab-indented columns", text_comm),
        case("paste", "paste joins corresponding lines with tabs, and -s -d joins one file's lines", text_paste),
        case("tee", "tee copies standard input to standard output and each file, and -a appends", text_tee),
        case("od", "od -A n -t x1 and -t o1 dump bytes in hexadecimal and octal", text_od),
        case("printf", "printf formats %s, %d, %5.2f, %x, %o and %c", text_printf),
        case("echo", "echo writes its arguments separated by single spaces and a newline", text_echo),
    ]),
    category("process-utils", "process utilities", &[
        case("sleep", "sleep 1 suspends execution for at least a second", proc_sleep),
        case("sleep-invalid", "sleep with an operand that is not a number writes a diagnostic and exits non-zero", proc_sleep_invalid),
        case("kill-l", "kill -l names the signal for a number and for an exit status above 128", proc_kill_l),
        case("kill-term", "kill sends SIGTERM by default, and wait reports 128 plus 15", proc_kill_term),
        case("kill-s", "kill -s KILL sends the named signal", proc_kill_s),
        case("kill-0", "kill -0 tests that a process exists without signalling it", proc_kill_0),
        case("wait-all", "wait with no operands waits for every background job and returns 0", proc_wait_all),
        case("wait-pid", "wait with a PID returns that job's exit status", proc_wait_pid),
        case("background", "A command ended with & runs in the background while the shell goes on", proc_background),
        case("background-stdin", "Without job control a background command's standard input is /dev/null", proc_background_stdin),
        case("ps-self", "ps -o pid= -p lists the shell's own PID", proc_ps_self),
        case("ps-comm", "ps -o comm= names the command a process runs", proc_ps_comm),
        case("ps-all", "ps -A lists every process, the shell included", proc_ps_all),
        case("ps-select", "ps rejects an empty -p list, fails when it selects nothing, selects by real user, and writes four columns by default", proc_ps_select),
        case("nohup", "nohup runs a command with SIGHUP ignored", proc_nohup),
        case("env-assign", "env NAME=value adds the variable to the environment of the command it runs", proc_env_assign),
        case("env-i", "env -i starts from an empty environment", proc_env_i),
        case("nice", "nice -n 5 runs a command at a niceness raised by 5", proc_nice),
        case("timeout", "timeout ends a command that runs too long with status 124 and passes on a quick one's status", proc_timeout),
        case("timeout-s", "timeout -s KILL sends the named signal and exits 128 plus its number", proc_timeout_s),
        case("timeout-forward", "timeout passes SIGTERM it receives on to the command and exits with the command's status", proc_timeout_forward),
        case("timeout-k", "timeout -k sends SIGKILL after a fractional limit even when the timeout signal is SIGALRM", proc_timeout_k),
        case("true", "true found through PATH exits 0", proc_true),
        case("false", "false found through PATH exits non-zero", proc_false),
        case("xargs", "xargs passes words from standard input as arguments", proc_xargs),
        case("xargs-n", "xargs -n 2 passes at most two arguments per command", proc_xargs_n),
        case("xargs-i", "xargs -I replaces a string with each whole input line", proc_xargs_i),
        case("id", "id -u writes a numeric user ID and id writes uid= with it", proc_id),
        case("uname", "uname writes the system name, the same as uname -s, and uname -a starts with it", proc_uname),
        case("date-format", "date +format expands %Y, %m, %d, %H, %M, %S and %% from the current time", proc_date_format),
        case("expr-arith", "expr evaluates integer arithmetic with *, /, % and negative operands", proc_expr_arith),
        case("expr-match", "expr string : regex writes the \\( \\) match or the length matched", proc_expr_match),
        case("expr-status", "expr exits 1 when the result is zero or null and 0 otherwise", proc_expr_status),
        case("test-arith", "test compares integers with -eq, -ne, -gt, -ge, -lt and -le", proc_test_arith),
        case("test-utility", "test found through PATH exits 1 when its integer comparison is false", proc_test_utility),
        case("umask", "umask sets the mask new files are created under, and umask -S writes it symbolically", proc_umask),
        case("getconf", "getconf writes ARG_MAX of at least 4096 and a PATH that finds the standard utilities", proc_getconf),
        case("getconf-names", "getconf answers option, limits.h, processor and programming-environment names, and -v rejects an unsupported environment", proc_getconf_names),
        case("command-v", "command -v writes the path of a utility, the name of a built-in or function, and fails for neither", proc_command_v),
        case("time", "time -p runs a command, writes real, user and sys times to stderr and exits with its status", proc_time),
        case("time-signal", "time -p exits 128 plus the signal number when a signal ends the command", proc_time_signal),
        case("tty", "tty with standard input that is not a terminal writes not a tty and exits 1", proc_tty),
    ]),
    category("scripts", "real shell scripts", &[
        case("wordfreq", "A tr | sort | uniq -c | sort | head pipeline lists the three most frequent words", script_wordfreq),
        case("wordfreq-n", "The word-frequency pipeline takes the number of words to list as an argument", script_wordfreq_n),
        case("wordfreq-stdin", "The word-frequency pipeline reads standard input when its file operand is -", script_wordfreq_stdin),
        case("configure", "A configure-style probe finds tools with command -v and writes config.h", script_configure),
        case("configure-prefix", "The configure probe takes --prefix=DIR into config.h", script_configure_prefix),
        case("configure-bad-option", "The configure probe rejects an unknown option on stderr with status 1 and writes nothing", script_configure_bad_option),
        case("build", "A make-free build loop compiles every source into out/", script_build),
        case("build-incremental", "The build loop rebuilds only the object older than its source", script_build_incremental),
        case("build-fails", "The build loop stops with set -e when a step fails", script_build_fails),
        case("rotate", "A log rotator moves the log to .1 and leaves an empty log", script_rotate),
        case("rotate-keep", "Rotating three times keeps only the newest KEEP old logs", script_rotate_keep),
        case("rotate-missing", "The log rotator reports a missing log on stderr and exits 1", script_rotate_missing),
        case("csv-report", "An awk CSV report totals quantity and revenue per region and overall", script_csv_report),
        case("csv-report-header-only", "The CSV report of a file with only a header has no regions and a zero total", script_csv_report_header_only),
        case("walk", "A recursive shell function walks a directory tree depth first", script_walk),
        case("walk-spaces", "The tree walker keeps names with spaces intact", script_walk_spaces),
        case("walk-symlink", "The tree walker lists a symbolic link to a directory without following it", script_walk_symlink),
        case("template", "A here-document template expands variables, a loop's output and arithmetic", script_template),
        case("template-quoted", "A script written from a quoted here-document keeps its $ references for when it runs", script_template_quoted),
        case("getopts", "getopts parses flags and option-arguments, and shift leaves the operands", script_getopts),
        case("getopts-grouped", "getopts takes grouped flags and stops at --", script_getopts_grouped),
        case("getopts-unknown", "getopts reports an unknown option and the script exits 2 with its usage", script_getopts_unknown),
        case("getopts-missing-arg", "getopts reports an option missing its argument and the script exits 2 with its usage", script_getopts_missing_arg),
        case("trap-cleanup", "An EXIT trap removes the script's temporary directory when it ends normally", script_trap_cleanup),
        case("trap-cleanup-exit", "The EXIT trap runs on exit 3 and the script keeps status 3", script_trap_cleanup_exit),
        case("trap-cleanup-int", "On SIGINT the INT trap exits 130 and the EXIT trap still cleans up", script_trap_cleanup_int),
        case("source-lib", "A script sources a library file found next to it and calls its functions", script_source_lib),
        case("source-lib-die", "A library's die function writes to stderr and exits the sourcing script with 1", script_source_lib_die),
        case("source-lib-elsewhere", "The script finds its library through $0 when run from another directory", script_source_lib_elsewhere),
    ]),
]);

// POSIX sh language

fn shell_quote_single() -> CaseResult {
    sh(r#"x=v; printf '%s\n' 'a  $x "q"'"#).expect("a  $x \"q\"\n", 0, Empty)
}

fn shell_quote_double() -> CaseResult {
    sh(r#"x=v; printf '%s\n' "<$x> \$x \"q\" \\ '$x'""#).expect("<v> $x \"q\" \\ 'v'\n", 0, Empty)
}

fn shell_param_default() -> CaseResult {
    sh(r#"unset u; e=; echo "${u:-d1} ${e:-d2} ${e-d3}|${u-d4}""#).expect("d1 d2 |d4\n", 0, Empty)
}

fn shell_param_length() -> CaseResult {
    sh("x=hello; e=; echo ${#x} ${#e}").expect("5 0\n", 0, Empty)
}

fn shell_param_trim() -> CaseResult {
    sh("p=a/b/c.tar.gz; echo ${p#*/} ${p##*/} ${p%.*} ${p%%.*}")
        .expect("b/c.tar.gz c.tar.gz a/b/c.tar a/b/c\n", 0, Empty)
}

fn shell_cmdsub() -> CaseResult {
    sh(r#"echo "[$(printf 'a\n\n\n')]" "[`echo b`]" "$(echo "$(echo deep)")""#).expect("[a] [b] deep\n", 0, Empty)
}

fn shell_arith() -> CaseResult {
    sh("x=7; echo $((x * 6)) $(( (1+2) << 2 )) $((17 % 5)) $((-7 / 2)) $((0x10 + 010))")
        .expect("42 12 2 -3 24\n", 0, Empty)
}

fn shell_field_split_ifs() -> CaseResult {
    sh(r#"IFS=:; v=a:b::c; set -- $v; printf '[%s]' "$@"; echo"#).expect("[a][b][][c]\n", 0, Empty)
}

fn shell_glob() -> CaseResult {
    sh("echo *.c ?.h [!a].c").file("a.c", "").file("b.c", "").file("c.h", "").expect("a.c b.c c.h b.c\n", 0, Empty)
}

fn shell_expansion_order() -> CaseResult {
    sh(r#"p='*.c'; e='$p'; echo $p "$p" $e"#).file("a.c", "").file("b.c", "").expect("a.c b.c *.c $p\n", 0, Empty)
}

fn shell_heredoc() -> CaseResult {
    sh(r#"x=v
while IFS= read -r l; do echo "[$l]"; done <<EOF
$x \$x $(echo c)
EOF"#)
        .expect("[v $x c]\n", 0, Empty)
}

fn shell_noclobber() -> CaseResult {
    sh("set -C; echo a > f; echo b > f || echo refused; echo c >| f; read l < f; echo $l")
        .expect("refused\nc\n", 0, Written)
}

fn shell_fd_dup() -> CaseResult {
    sh("{ echo out; echo err >&2; } 2>&1").expect("out\nerr\n", 0, Empty)
}

fn shell_pipeline() -> CaseResult {
    sh(r#"false | true; echo $?; true | false; echo $?; echo abc | { read x; echo "got $x"; }"#)
        .expect("0\n1\ngot abc\n", 0, Empty)
}

fn shell_pipeline_bang() -> CaseResult {
    sh("! false; echo $?; ! true; echo $?; ! true | false; echo $?").expect("0\n1\n0\n", 0, Empty)
}

fn shell_and_or() -> CaseResult {
    sh("false && echo no; true && echo yes; false || echo alt; true || echo no; false && echo a || echo b")
        .expect("yes\nalt\nb\n", 0, Empty)
}

fn shell_if_elif() -> CaseResult {
    sh("for v in 1 2 3; do if [ $v = 1 ]; then echo one; elif [ $v = 2 ]; then echo two; else echo other; fi; done")
        .expect("one\ntwo\nother\n", 0, Empty)
}

fn shell_case() -> CaseResult {
    sh("for w in apple b.c Zed '' 'x]'; do case $w in a*) echo A;; *.c|*.h) echo C;; [[:upper:]]*) echo U;; '') echo E;; *) echo other;; esac; done")
        .expect("A\nC\nU\nE\nother\n", 0, Empty)
}

fn shell_for_loop() -> CaseResult {
    sh(r#"set -- x 'y z'; for a; do echo "[$a]"; done; for i in 1 2 3; do printf %s $i; done; echo"#)
        .expect("[x]\n[y z]\n123\n", 0, Empty)
}

fn shell_while_until() -> CaseResult {
    sh("i=0; while [ $i -lt 3 ]; do i=$((i+1)); done; until [ $i -eq 0 ]; do printf %s $i; i=$((i-1)); done; echo")
        .expect("321\n", 0, Empty)
}

fn shell_function_positional() -> CaseResult {
    sh(r#"set -- outer; f() { echo "$# $1|$2"; }; f a 'b c'; echo "$# $1""#).expect("2 a|b c\n1 outer\n", 0, Empty)
}

fn shell_function_return() -> CaseResult {
    sh("f() { return 3; echo no; }; f; echo $?; g() { false; }; g; echo $?").expect("3\n1\n", 0, Empty)
}

fn shell_special_params() -> CaseResult {
    sh(r#"set -- a 'b c' d; echo $#; for x in "$@"; do echo "[$x]"; done; IFS=,; echo "$*""#)
        .expect("3\n[a]\n[b c]\n[d]\na,b c,d\n", 0, Empty)
}

fn shell_special_pid() -> CaseResult {
    sh(r#"a=$$; b=$(echo $$); c=$( (echo $$) ); [ "$a" = "$b" ] && [ "$a" = "$c" ] && echo same; (exit 7) & p=$!; wait $p; echo $?"#)
        .expect("same\n7\n", 0, Empty)
}

fn shell_set_e() -> CaseResult {
    sh("set -e; if false; then :; fi; false || true; ! true; echo ok; false; echo no").expect("ok\n", 1, Empty)
}

fn shell_set_u() -> CaseResult {
    sh(r#"set -u; echo ${zz-d}; echo "$zz"; echo no"#).expect("d\n", NONZERO, Written)
}

fn shell_set_f() -> CaseResult {
    sh("set -f; echo *.c; set +f; echo *.c").file("a.c", "").expect("*.c\na.c\n", 0, Empty)
}

fn shell_export() -> CaseResult {
    sh(r#"x=1; export y=2; sh -c 'echo "[$x][$y]"'; z=3 sh -c 'echo $z'; echo "[$z]""#)
        .needs(&["sh"])
        .expect("[][2]\n3\n[]\n", 0, Empty)
}

fn shell_readonly() -> CaseResult {
    sh("readonly r=1; (r=2; echo no) || echo refused; echo $r").expect("refused\n1\n", 0, Written)
}

fn shell_unset() -> CaseResult {
    sh(r#"x=1; unset x; echo "[${x-unset}]"; f() { :; }; unset -f f; f 2>/dev/null || echo gone"#)
        .expect("[unset]\ngone\n", 0, Empty)
}

fn shell_shift() -> CaseResult {
    sh("set -- a b c d; shift; echo $1; shift 2; echo $# $1").expect("b\n1 d\n", 0, Empty)
}

fn shell_eval() -> CaseResult {
    sh(r#"cmd='x=5; echo $((x+1))'; eval "$cmd"; v=y; eval "$v=7"; echo $y"#).expect("6\n7\n", 0, Empty)
}

fn shell_exec() -> CaseResult {
    sh("exec sh -c 'echo replaced'; echo never").needs(&["sh"]).expect("replaced\n", 0, Empty)
}

fn shell_trap_exit() -> CaseResult {
    sh("trap 'echo bye' EXIT; echo hi; exit 3").expect("hi\nbye\n", 3, Empty)
}

fn shell_trap_signal() -> CaseResult {
    sh("trap 'echo got' USR1; kill -USR1 $$; echo after").expect("got\nafter\n", 0, Empty)
}

fn shell_exit() -> CaseResult {
    sh("(exit 3); echo $?; exit 4; echo no").expect("3\n", 4, Empty)
}

fn shell_dot() -> CaseResult {
    sh(". ./lib.sh; echo $?; echo $v; f x")
        .file("lib.sh", r#"v=fromlib
f() { echo f:$1; }
return 5
echo unreachable
"#)
        .expect("5\nfromlib\nf:x\n", 0, Empty)
}

fn shell_cd_pwd() -> CaseResult {
    sh(r#"cd d && echo ${PWD##*/}; cd ..; [ "$PWD" = "$(pwd)" ] && echo same; cd /nonexistent 2>/dev/null || echo nocd"#)
        .dir("d")
        .expect("d\nsame\nnocd\n", 0, Empty)
}

fn shell_signal_status() -> CaseResult {
    sh("{ sh -c 'kill -TERM $$'; echo $?; sh -c 'kill -KILL $$'; echo $?; } 2>/dev/null")
        .needs(&["sh"])
        .expect("143\n137\n", 0, Empty)
}

fn shell_subshell() -> CaseResult {
    sh(r#"x=1; (x=2; cd /; echo $x); echo $x; { x=3; }; echo $x; [ "$PWD" != / ] && echo here"#)
        .expect("2\n1\n3\nhere\n", 0, Empty)
}

// file utilities

fn file_ls() -> CaseResult {
    util(&["ls"]).file("b", "").file("a", "").file(".hidden", "").expect("a\nb\n", 0, Empty)
}

fn file_ls_a() -> CaseResult {
    util(&["ls", "-a"]).file("b", "").file("a", "").file(".hidden", "").expect(".\n..\n.hidden\na\nb\n", 0, Empty)
}

fn file_ls_d() -> CaseResult {
    sh("ls d; ls -d d").needs(&["ls"]).dir("d").file("d/x", "").expect("x\nd\n", 0, Empty)
}

fn file_ls_l() -> CaseResult {
    sh("set -- $(ls -l f); echo $1 $2 $5 $9")
        .needs(&["ls"])
        .file_mode("f", "abc", 0o640)
        .expect("-rw-r----- 1 3 f\n", 0, Empty)
}

fn file_ls_l_dir() -> CaseResult {
    sh("set -- $(ls -ld d); echo $1 $9").needs(&["ls"]).dir("d").expect("drwxr-xr-x d\n", 0, Empty)
}

fn file_cp() -> CaseResult {
    sh("cp a b && cat b")
        .needs(&["cp", "cat"])
        .file("a", r#"one
two
"#)
        .expect("one\ntwo\n", 0, Empty)
}

fn file_cp_r() -> CaseResult {
    sh("cp -R d e && cat e/x e/sub/y && [ -d e/sub ] && echo dir")
        .needs(&["cp", "cat"])
        .dir("d")
        .dir("d/sub")
        .file("d/x", r#"X
"#)
        .file("d/sub/y", r#"Y
"#)
        .expect("X\nY\ndir\n", 0, Empty)
}

fn file_cp_dir_needs_r() -> CaseResult {
    util(&["cp", "d", "e"]).dir("d").expect("", NONZERO, Written)
}

fn file_cp_p() -> CaseResult {
    sh("touch -t 200001020304.05 a && cp -p a b && { [ b -nt a ] || [ b -ot a ] || echo same-time; }; set -- $(ls -l b); echo $1")
        .needs(&["cp", "touch", "ls"])
        .file_mode("a", r#"x
"#, 0o600)
        .expect("same-time\n-rw-------\n", 0, Empty)
}

fn file_mv_across() -> CaseResult {
    sh("mv d1/f d2/ && [ ! -e d1/f ] && cat d2/f")
        .needs(&["mv", "cat"])
        .dir("d1")
        .dir("d2")
        .file("d1/f", r#"F
"#)
        .expect("F\n", 0, Empty)
}

fn file_mv_dir() -> CaseResult {
    sh("mv d1 d3 && [ ! -e d1 ] && cat d3/f")
        .needs(&["mv", "cat"])
        .dir("d1")
        .file("d1/f", r#"F
"#)
        .expect("F\n", 0, Empty)
}

fn file_rm_r() -> CaseResult {
    sh("rm -r d && [ ! -e d ] && echo gone")
        .needs(&["rm"])
        .dir("d")
        .dir("d/s")
        .file("d/a.txt", r#"A
"#)
        .file("d/b.log", r#"B
"#)
        .file("d/s/c.txt", r#"C
"#)
        .expect("gone\n", 0, Empty)
}

fn file_rm_missing() -> CaseResult {
    util(&["rm", "missing"]).expect("", NONZERO, Written)
}

fn file_rm_f() -> CaseResult {
    util(&["rm", "-f", "missing"]).expect("", 0, Empty)
}

fn file_mkdir_p() -> CaseResult {
    sh("mkdir -p x/y/z && [ -d x/y/z ] && mkdir -p x/y && echo ok").needs(&["mkdir"]).expect("ok\n", 0, Empty)
}

fn file_rmdir() -> CaseResult {
    sh("rmdir e && [ ! -e e ] && echo ok").needs(&["rmdir"]).dir("e").expect("ok\n", 0, Empty)
}

fn file_rmdir_nonempty() -> CaseResult {
    sh("rmdir d; s=$?; [ $s -ne 0 ] && [ -f d/x ] && echo kept")
        .needs(&["rmdir"])
        .dir("d")
        .file("d/x", "")
        .expect("kept\n", 0, Written)
}

fn file_ln() -> CaseResult {
    sh("ln a h && echo two >> h && cat a; set -- $(ls -l a); echo $2")
        .needs(&["ln", "cat", "ls"])
        .file("a", r#"one
"#)
        .expect("one\ntwo\n2\n", 0, Empty)
}

fn file_ln_s() -> CaseResult {
    sh("ln -s a s && [ -h s ] && cat s")
        .needs(&["ln", "cat"])
        .file("a", r#"target
"#)
        .expect("target\n", 0, Empty)
}

fn file_chmod_octal() -> CaseResult {
    sh("chmod 751 a && set -- $(ls -l a) && echo $1")
        .needs(&["chmod", "ls"])
        .file("a", "")
        .expect("-rwxr-x--x\n", 0, Empty)
}

fn file_chmod_symbolic() -> CaseResult {
    sh("chmod u+x,g-r,o=w a && set -- $(ls -l a) && echo $1")
        .needs(&["chmod", "ls"])
        .file_mode("a", "", 0o644)
        .expect("-rwx----w-\n", 0, Empty)
}

fn file_touch_create() -> CaseResult {
    sh("touch new && [ -f new ] && [ ! -s new ] && echo empty").needs(&["touch"]).expect("empty\n", 0, Empty)
}

fn file_touch_t() -> CaseResult {
    sh("TZ=UTC0; export TZ; touch -t 200001020304.05 a && set -- $(ls -l a) && echo $6 $7 $8")
        .needs(&["touch", "ls"])
        .file("a", "")
        .expect("Jan 2 2000\n", 0, Empty)
}

fn file_test_nt() -> CaseResult {
    sh("touch -t 200001010000 old && [ new -nt old ] && [ old -ot new ] && echo newer")
        .needs(&["touch"])
        .file("new", "")
        .expect("newer\n", 0, Empty)
}

fn file_find_name() -> CaseResult {
    sh("find d -name '*.txt' | sort")
        .needs(&["find", "sort"])
        .dir("d")
        .dir("d/s")
        .file("d/a.txt", r#"A
"#)
        .file("d/b.log", r#"B
"#)
        .file("d/s/c.txt", r#"C
"#)
        .expect("d/a.txt\nd/s/c.txt\n", 0, Empty)
}

fn file_find_type() -> CaseResult {
    sh("find d -type d | sort")
        .needs(&["find", "sort"])
        .dir("d")
        .dir("d/s")
        .file("d/a.txt", r#"A
"#)
        .file("d/b.log", r#"B
"#)
        .file("d/s/c.txt", r#"C
"#)
        .expect("d\nd/s\n", 0, Empty)
}

fn file_find_mtime() -> CaseResult {
    sh("touch -t 200001010000 d/a.txt && find d -type f -mtime +1000")
        .needs(&["find", "touch"])
        .dir("d")
        .dir("d/s")
        .file("d/a.txt", r#"A
"#)
        .file("d/b.log", r#"B
"#)
        .file("d/s/c.txt", r#"C
"#)
        .expect("d/a.txt\n", 0, Empty)
}

fn file_find_exec() -> CaseResult {
    sh(r#"find d -name '*.log' -exec cat {} \;"#)
        .needs(&["find", "cat"])
        .dir("d")
        .dir("d/s")
        .file("d/a.txt", r#"A
"#)
        .file("d/b.log", r#"B
"#)
        .file("d/s/c.txt", r#"C
"#)
        .expect("B\n", 0, Empty)
}

fn file_find_prune() -> CaseResult {
    sh("find d -name s -prune -o -type f -print | sort")
        .needs(&["find", "sort"])
        .dir("d")
        .dir("d/s")
        .file("d/a.txt", r#"A
"#)
        .file("d/b.log", r#"B
"#)
        .file("d/s/c.txt", r#"C
"#)
        .expect("d/a.txt\nd/b.log\n", 0, Empty)
}

fn file_du() -> CaseResult {
    sh("du d | while read n p; do case $n in *[!0-9]*) echo bad;; esac; echo $p; done | sort")
        .needs(&["du", "sort"])
        .dir("d")
        .dir("d/s")
        .file("d/a.txt", r#"A
"#)
        .file("d/b.log", r#"B
"#)
        .file("d/s/c.txt", r#"C
"#)
        .expect("d\nd/s\n", 0, Empty)
}

fn file_df() -> CaseResult {
    sh("df -k -P . | { read a b c; echo $a $b; read fs n rest; case $n in ''|*[!0-9]*) echo bad;; *) echo blocks;; esac; }")
        .needs(&["df"])
        .expect("Filesystem 1024-blocks\nblocks\n", 0, Empty)
}

fn file_test_file_ops() -> CaseResult {
    sh("ln -s f l; for t in '-e f' '-f f' '-d d' '-s f' '-s z' '-r f' '-w f' '-x x' '-x f' '-h l' '-h f' '-e nope' '-f d'; do if test $t; then printf 1; else printf 0; fi; done; echo")
        .needs(&["ln"])
        .dir("d")
        .file("f", "x")
        .file("z", "")
        .file_mode("x", "", 0o755)
        .expect("1111011101000\n", 0, Empty)
}

fn file_bracket_utility() -> CaseResult {
    util(&["[", "-d", "d", "]"]).dir("d").expect("", 0, Empty)
}

fn file_basename() -> CaseResult {
    sh("basename /a/b/c.txt .txt; basename a/b/; basename /").needs(&["basename"]).expect("c\nb\n/\n", 0, Empty)
}

fn file_dirname() -> CaseResult {
    sh("dirname /a/b/c; dirname a; dirname /; dirname /a").needs(&["dirname"]).expect("/a/b\n.\n/\n/\n", 0, Empty)
}

fn file_readlink() -> CaseResult {
    sh("ln -s some/target s && readlink s").needs(&["ln", "readlink"]).expect("some/target\n", 0, Empty)
}

fn file_realpath() -> CaseResult {
    sh(r#"ln -s x/y l; [ "$(realpath x/y/..)" = "$(pwd -P)/x" ] && [ "$(realpath l)" = "$(pwd -P)/x/y" ] && echo ok"#)
        .needs(&["ln", "realpath"])
        .dir("x")
        .dir("x/y")
        .expect("ok\n", 0, Empty)
}

// text utilities

fn text_cat() -> CaseResult {
    util(&["cat", "a", "-", "b"])
        .file("a", r#"A
"#)
        .file("b", r#"B
"#)
        .input(r#"S
"#)
        .expect("A\nS\nB\n", 0, Empty)
}

fn text_head_n() -> CaseResult {
    util(&["head", "-n", "2"])
        .input(r#"1
2
3
4
"#)
        .expect("1\n2\n", 0, Empty)
}

fn text_head_c() -> CaseResult {
    util(&["head", "-c", "5"]).input("abcdefgh").expect("abcde", 0, Empty)
}

fn text_tail_n() -> CaseResult {
    util(&["tail", "-n", "2"])
        .input(r#"1
2
3
4
"#)
        .expect("3\n4\n", 0, Empty)
}

fn text_wc() -> CaseResult {
    sh("set -- $(wc < f); echo $1 $2 $3")
        .needs(&["wc"])
        .file("f", r#"one two
three
"#)
        .expect("2 3 14\n", 0, Empty)
}

fn text_sort() -> CaseResult {
    util(&["sort"])
        .input(r#"b
A
a
B
"#)
        .expect("A\nB\na\nb\n", 0, Empty)
}

fn text_sort_n() -> CaseResult {
    util(&["sort", "-n"])
        .input(r#"10
9
-1
100
"#)
        .expect("-1\n9\n10\n100\n", 0, Empty)
}

fn text_sort_r() -> CaseResult {
    util(&["sort", "-r"])
        .input(r#"a
c
b
"#)
        .expect("c\nb\na\n", 0, Empty)
}

fn text_sort_k() -> CaseResult {
    util(&["sort", "-k", "2,2n"])
        .input(r#"x 10
y 9
z 100
"#)
        .expect("y 9\nx 10\nz 100\n", 0, Empty)
}

fn text_sort_u() -> CaseResult {
    util(&["sort", "-u"])
        .input(r#"b
a
b
a
"#)
        .expect("a\nb\n", 0, Empty)
}

fn text_uniq_c() -> CaseResult {
    sh(r#"uniq -c | while read n l; do echo "$n:$l"; done"#)
        .needs(&["uniq"])
        .input(r#"a
a
b
"#)
        .expect("2:a\n1:b\n", 0, Empty)
}

fn text_cut_f() -> CaseResult {
    util(&["cut", "-d", ":", "-f", "1,3"])
        .input(r#"a:b:c:d
"#)
        .expect("a:c\n", 0, Empty)
}

fn text_cut_c() -> CaseResult {
    util(&["cut", "-c", "2-4"])
        .input(r#"abcdef
"#)
        .expect("bcd\n", 0, Empty)
}

fn text_tr_range() -> CaseResult {
    util(&["tr", "a-z", "A-Z"])
        .input(r#"Hello
"#)
        .expect("HELLO\n", 0, Empty)
}

fn text_tr_d() -> CaseResult {
    util(&["tr", "-d", "0-9"])
        .input(r#"a1b22c
"#)
        .expect("abc\n", 0, Empty)
}

fn text_tr_s() -> CaseResult {
    util(&["tr", "-s", " "])
        .input(r#"a   b  c
"#)
        .expect("a b c\n", 0, Empty)
}

fn text_sed_s() -> CaseResult {
    util(&["sed", "s/o/0/;s/b/B/g"])
        .input(r#"foo bob
"#)
        .expect("f0o BoB\n", 0, Empty)
}

fn text_sed_n_p() -> CaseResult {
    util(&["sed", "-n", "2p;$p"])
        .input(r#"1
2
3
4
"#)
        .expect("2\n4\n", 0, Empty)
}

fn text_sed_range() -> CaseResult {
    util(&["sed", "2,3d"])
        .input(r#"1
2
3
4
"#)
        .expect("1\n4\n", 0, Empty)
}

fn text_awk_fields() -> CaseResult {
    util(&["awk", "-F", ":", "{print $2, NF}"])
        .input(r#"a:b:c
"#)
        .expect("b 3\n", 0, Empty)
}

fn text_awk_pattern() -> CaseResult {
    util(&["awk", "$2 > 1 {print $1}"])
        .input(r#"x 1
y 2
z 3
"#)
        .expect("y\nz\n", 0, Empty)
}

fn text_awk_begin_end() -> CaseResult {
    util(&["awk", "BEGIN {n = 10} {n += $1} END {print n, NR}"])
        .input(r#"1
2
3
"#)
        .expect("16 3\n", 0, Empty)
}

fn text_awk_printf() -> CaseResult {
    util(&["awk", r#"{printf "%5.2f|%-3s|%d|%x\n", $1, $2, $1, 255}"#])
        .input(r#"3.14159 ab
"#)
        .expect(" 3.14|ab |3|ff\n", 0, Empty)
}

fn text_grep_e() -> CaseResult {
    util(&["grep", "-E", "^(a|b)x"])
        .input(r#"ax
bx
cx
"#)
        .expect("ax\nbx\n", 0, Empty)
}

fn text_grep_v() -> CaseResult {
    util(&["grep", "-v", "x"])
        .input(r#"ax
b
"#)
        .expect("b\n", 0, Empty)
}

fn text_grep_c() -> CaseResult {
    util(&["grep", "-c", "a"])
        .input(r#"a
b
a
"#)
        .expect("2\n", 0, Empty)
}

fn text_grep_l() -> CaseResult {
    util(&["grep", "-l", "match", "f1", "f2", "f3"])
        .file("f1", r#"match
"#)
        .file("f2", r#"no
"#)
        .file("f3", r#"a match
"#)
        .expect("f1\nf3\n", 0, Empty)
}

fn text_grep_n() -> CaseResult {
    util(&["grep", "-n", "b"])
        .input(r#"a
b
c
b
"#)
        .expect("2:b\n4:b\n", 0, Empty)
}

fn text_grep_i() -> CaseResult {
    util(&["grep", "-i", "abc"])
        .input(r#"ABC
abc
xyz
"#)
        .expect("ABC\nabc\n", 0, Empty)
}

fn text_grep_status() -> CaseResult {
    sh("echo a | grep b; echo $?; grep a missing; [ $? -gt 1 ] && echo error")
        .needs(&["grep"])
        .expect("1\nerror\n", 0, Written)
}

fn text_diff_same() -> CaseResult {
    util(&["diff", "a", "b"])
        .file("a", r#"1
2
"#)
        .file("b", r#"1
2
"#)
        .expect("", 0, Empty)
}

fn text_diff_differ() -> CaseResult {
    util(&["diff", "a", "b"])
        .file("a", r#"1
2
3
"#)
        .file("b", r#"1
x
3
"#)
        .expect("2c2\n< 2\n---\n> x\n", 1, Empty)
}

fn text_cmp() -> CaseResult {
    util(&["cmp", "a", "b"])
        .file("a", r#"abc
"#)
        .file("b", r#"abd
"#)
        .expect("a b differ: char 3, line 1\n", 1, Empty)
}

fn text_comm() -> CaseResult {
    util(&["comm", "a", "b"])
        .file("a", r#"a
b
c
"#)
        .file("b", r#"b
c
d
"#)
        .expect("a\n\t\tb\n\t\tc\n\td\n", 0, Empty)
}

fn text_paste() -> CaseResult {
    sh("paste a b; paste -s -d , a")
        .needs(&["paste"])
        .file("a", r#"1
2
"#)
        .file("b", r#"x
y
"#)
        .expect("1\tx\n2\ty\n1,2\n", 0, Empty)
}

fn text_tee() -> CaseResult {
    sh("echo x | tee f1; echo y | tee -a f2 >/dev/null; cat f1 f2")
        .needs(&["tee", "cat"])
        .file("f2", r#"old
"#)
        .expect("x\nx\nold\ny\n", 0, Empty)
}

fn text_od() -> CaseResult {
    sh(r#"printf 'AB\n' | od -A n -t x1 | { read a b c; echo $a $b $c; }; printf 'AB\n' | od -A n -t o1 | { read a b c; echo $a $b $c; }"#)
        .needs(&["od"])
        .expect("41 42 0a\n101 102 012\n", 0, Empty)
}

fn text_printf() -> CaseResult {
    util(&["printf", r#"%s-%d-%5.2f|%x|%o|%c\n"#, "x", "5", "3.14159", "255", "8", "abc"])
        .expect("x-5- 3.14|ff|10|a\n", 0, Empty)
}

fn text_echo() -> CaseResult {
    util(&["echo", "a", "b  c"]).expect("a b  c\n", 0, Empty)
}

// process utilities

fn proc_sleep() -> CaseResult {
    sh("s=$(date +%s); sleep 1; e=$(date +%s); [ $((e - s)) -ge 1 ] && echo waited")
        .needs(&["sleep", "date"])
        .expect("waited\n", 0, Empty)
}

fn proc_sleep_invalid() -> CaseResult {
    util(&["sleep", "abc"]).expect("", NONZERO, Written)
}

fn proc_kill_l() -> CaseResult {
    sh("kill -l 9; kill -l 15; kill -l 143").expect("KILL\nTERM\nTERM\n", 0, Empty)
}

fn proc_kill_term() -> CaseResult {
    sh("sleep 5 & p=$!; kill $p; { wait $p; echo $?; } 2>/dev/null").needs(&["sleep"]).expect("143\n", 0, Empty)
}

fn proc_kill_s() -> CaseResult {
    sh("sleep 5 & p=$!; kill -s KILL $p; { wait $p; echo $?; } 2>/dev/null").needs(&["sleep"]).expect("137\n", 0, Empty)
}

fn proc_kill_0() -> CaseResult {
    sh("kill -0 $$ && echo alive; (exit 0) & p=$!; wait $p; kill -0 $p 2>/dev/null || echo gone")
        .expect("alive\ngone\n", 0, Empty)
}

fn proc_wait_all() -> CaseResult {
    sh("(exit 3) & (exit 4) & wait; echo $?").expect("0\n", 0, Empty)
}

fn proc_wait_pid() -> CaseResult {
    sh("(exit 3) & a=$!; (exit 4) & b=$!; wait $b; echo $?; wait $a; echo $?").expect("4\n3\n", 0, Empty)
}

fn proc_background() -> CaseResult {
    sh("{ sleep 1; echo late; } & echo early; wait").needs(&["sleep"]).expect("early\nlate\n", 0, Empty)
}

fn proc_background_stdin() -> CaseResult {
    sh("echo data | { cat & wait; }; echo done").needs(&["cat"]).expect("done\n", 0, Empty)
}

fn proc_ps_self() -> CaseResult {
    sh(r#"set -- $(ps -o pid= -p $$); [ "$1" = $$ ] && echo self"#).needs(&["ps"]).expect("self\n", 0, Empty)
}

fn proc_ps_comm() -> CaseResult {
    // ps is not the script's last command, so the shell runs it as a child
    // instead of exec'ing it, and $$ is still the shell.
    sh("ps -o comm= -p $$; :").needs(&["ps"]).expect("sh\n", 0, Empty)
}

fn proc_ps_all() -> CaseResult {
    sh(r#"ps -A -o pid= | { f=no; while read p; do [ "$p" = $$ ] && f=yes; done; echo $f; }"#)
        .needs(&["ps"])
        .expect("yes\n", 0, Empty)
}

fn proc_ps_select() -> CaseResult {
    sh(r#"ps -o pid= -p '' 2>/dev/null || echo empty; ps -o pid= -p 99999 || echo none; ps -o pid= -U $(id -ru) | { f=no; while read p; do [ "$p" = $$ ] && f=yes; done; echo $f; }; ps | { read a b c d e; [ -n "$d" ] && [ -z "$e" ] && echo four; }"#)
        .needs(&["ps", "id"])
        .expect("empty\nnone\nyes\nfour\n", 0, Empty)
}

fn proc_nohup() -> CaseResult {
    util(&["nohup", "sh", "-c", "kill -HUP $$; echo survived"]).expect("survived\n", 0, Empty)
}

fn proc_env_assign() -> CaseResult {
    util(&["env", "X=1", "sh", "-c", "echo $X"]).expect("1\n", 0, Empty)
}

fn proc_env_i() -> CaseResult {
    util(&["env", "-i", "Y=2", "env"]).expect("Y=2\n", 0, Empty)
}

fn proc_nice() -> CaseResult {
    sh("nice -n 5 sh -c 'ps -o nice= -p $$' | { read n; echo $n; }").needs(&["nice", "ps"]).expect("5\n", 0, Empty)
}

fn proc_timeout() -> CaseResult {
    sh("timeout 1 sleep 5; echo $?; timeout 5 sh -c 'exit 3'; echo $?")
        .needs(&["timeout", "sleep"])
        .expect("124\n3\n", 0, Empty)
}

fn proc_timeout_s() -> CaseResult {
    sh("{ timeout -s KILL 1 sleep 5; echo $?; } 2>/dev/null").needs(&["timeout", "sleep"]).expect("137\n", 0, Empty)
}

fn proc_timeout_forward() -> CaseResult {
    sh(r#"timeout 10 sh -c 'trap "echo got; exit 7" TERM; sleep 5 & wait' & t=$!; sleep 1; kill $t; wait $t; echo $?"#)
        .needs(&["timeout", "sleep"])
        .expect("got\n7\n", 0, Empty)
}

fn proc_timeout_k() -> CaseResult {
    sh(r#"{ timeout -s ALRM -k 0.5 0.5 sh -c 'trap "" ALRM; sleep 5'; echo $?; } 2>/dev/null"#)
        .needs(&["timeout", "sleep"])
        .expect("137\n", 0, Empty)
}

fn proc_true() -> CaseResult {
    util(&["true"]).expect("", 0, Empty)
}

fn proc_false() -> CaseResult {
    util(&["false"]).expect("", NONZERO, Empty)
}

fn proc_xargs() -> CaseResult {
    util(&["xargs", "echo"])
        .input(r#"a b
c
"#)
        .expect("a b c\n", 0, Empty)
}

fn proc_xargs_n() -> CaseResult {
    util(&["xargs", "-n", "2", "echo"])
        .input(r#"1 2 3
"#)
        .expect("1 2\n3\n", 0, Empty)
}

fn proc_xargs_i() -> CaseResult {
    util(&["xargs", "-I", "{}", "echo", "<{}>"])
        .input(r#"a b
c
"#)
        .expect("<a b>\n<c>\n", 0, Empty)
}

fn proc_id() -> CaseResult {
    sh(r#"u=$(id -u); case $u in ''|*[!0-9]*) echo bad;; *) echo uid;; esac; case $(id) in "uid=$u("*|"uid=$u "*) echo form;; esac"#)
        .needs(&["id"])
        .expect("uid\nform\n", 0, Empty)
}

fn proc_uname() -> CaseResult {
    sh(r#"s=$(uname -s); [ -n "$s" ] && [ "$(uname)" = "$s" ] && case "$(uname -a)" in "$s "*) echo ok;; esac"#)
        .needs(&["uname"])
        .expect("ok\n", 0, Empty)
}

fn proc_date_format() -> CaseResult {
    sh("case $(date -u '+%Y-%m-%dT%H:%M:%S%%') in [0-9][0-9][0-9][0-9]-[01][0-9]-[0-3][0-9]T[0-2][0-9]:[0-5][0-9]:[0-6][0-9]%) echo ok;; esac")
        .needs(&["date"])
        .expect("ok\n", 0, Empty)
}

fn proc_expr_arith() -> CaseResult {
    sh(r#"expr 6 \* 7; expr 7 / 2; expr 7 % 3; expr -5 + 2"#).needs(&["expr"]).expect("42\n3\n1\n-3\n", 0, Empty)
}

fn proc_expr_match() -> CaseResult {
    sh(r#"expr abcde : 'a\(.*\)d'; expr abc : '.*'"#).needs(&["expr"]).expect("bc\n3\n", 0, Empty)
}

fn proc_expr_status() -> CaseResult {
    sh(r#"expr 1 = 2; echo $?; expr 2 \> 1; echo $?"#).needs(&["expr"]).expect("0\n1\n1\n0\n", 0, Empty)
}

fn proc_test_arith() -> CaseResult {
    sh("for t in '10 -gt 9' '-3 -lt 2' '5 -eq 05' '2 -ge 3' '4 -ne 4' '1 -le 1'; do if test $t; then printf 1; else printf 0; fi; done; echo")
        .expect("111001\n", 0, Empty)
}

fn proc_test_utility() -> CaseResult {
    util(&["test", "10", "-lt", "2"]).expect("", 1, Empty)
}

fn proc_umask() -> CaseResult {
    sh("umask 027; touch f; set -- $(ls -l f); echo $1; umask -S")
        .needs(&["touch", "ls"])
        .expect("-rw-r-----\nu=rwx,g=rx,o=\n", 0, Empty)
}

fn proc_getconf() -> CaseResult {
    sh("[ $(getconf ARG_MAX) -ge 4096 ] && echo ok; case $(getconf PATH) in */bin*) echo path;; esac")
        .needs(&["getconf"])
        .expect("ok\npath\n", 0, Empty)
}

fn proc_getconf_names() -> CaseResult {
    sh(r#"[ $(getconf _POSIX_VERSION) -ge 200809 ] && echo version; getconf -v POSIX_V7_LP64_OFF64 _POSIX_V7_LP64_OFF64; getconf INT_MAX; [ $(getconf NPROCESSORS_ONLN) -ge 1 ] && echo cpus; [ $(getconf OPEN_MAX) -ge 20 ] && echo open; getconf POSIX_V7_LP64_OFF64_CFLAGS >/dev/null && echo flags; getconf -v POSIX_V7_ILP32_OFF32 PATH 2>/dev/null || echo unsupported"#)
        .needs(&["getconf"])
        .expect("version\n1\n2147483647\ncpus\nopen\nflags\nunsupported\n", 0, Empty)
}

fn proc_command_v() -> CaseResult {
    sh("command -v sh; command -v cd; f() { :; }; command -v f; command -v nonexistent_zz || echo none")
        .expect("/bin/sh\ncd\nf\nnone\n", 0, Empty)
}

fn proc_time() -> CaseResult {
    sh("time -p sh -c 'exit 3' 2> t; echo $?; while read a b; do echo $a; done < t")
        .needs(&["time"])
        .expect("3\nreal\nuser\nsys\n", 0, Empty)
}

fn proc_time_signal() -> CaseResult {
    sh("time -p sh -c 'kill -9 $$' 2>/dev/null; echo $?").needs(&["time"]).expect("137\n", 0, Empty)
}

fn proc_tty() -> CaseResult {
    util(&["tty"]).expect("not a tty\n", 1, Empty)
}

// real shell scripts

fn script_wordfreq() -> CaseResult {
    util(&["sh", "wordfreq.sh", "text.txt"])
        .needs(&["cat", "tr", "sort", "uniq", "head"])
        .file("wordfreq.sh", WORDFREQ)
        .file("text.txt", r#"The cat and the hat. The end, and THE cat!
"#)
        .expect("the 4\nand 2\ncat 2\n", 0, Empty)
}

fn script_wordfreq_n() -> CaseResult {
    util(&["sh", "wordfreq.sh", "text.txt", "5"])
        .needs(&["cat", "tr", "sort", "uniq", "head"])
        .file("wordfreq.sh", WORDFREQ)
        .file("text.txt", r#"The cat and the hat. The end, and THE cat!
"#)
        .expect("the 4\nand 2\ncat 2\nend 1\nhat 1\n", 0, Empty)
}

fn script_wordfreq_stdin() -> CaseResult {
    util(&["sh", "wordfreq.sh", "-", "2"])
        .needs(&["cat", "tr", "sort", "uniq", "head"])
        .file("wordfreq.sh", WORDFREQ)
        .input(r#"b a b c b a
"#)
        .expect("b 3\na 2\n", 0, Empty)
}

fn script_configure() -> CaseResult {
    sh("sh configure.sh && cat config.h && [ ! -e config.h.tmp ] && [ ! -e conftest.c ] && echo clean")
        .needs(&["sh", "tr", "cat", "rm", "mv", "sort", "awk"])
        .file("configure.sh", CONFIGURE)
        .expect("configure: wrote config.h\n#define HAVE_SH 1\n#define HAVE_SORT 1\n#define HAVE_AWK 1\n/* #undef HAVE_NO_SUCH_TOOL_ZZ */\n#define HAVE_CONFTEST 1\n#define PREFIX \"/usr/local\"\nclean\n", 0, Empty)
}

fn script_configure_prefix() -> CaseResult {
    sh("sh configure.sh --prefix=/opt/x >/dev/null && tail -n 1 config.h")
        .needs(&["sh", "tr", "cat", "rm", "mv", "sort", "awk"])
        .file("configure.sh", CONFIGURE)
        .expect("#define PREFIX \"/opt/x\"\n", 0, Empty)
}

fn script_configure_bad_option() -> CaseResult {
    sh("sh configure.sh --frobnicate; echo $?; [ -e config.h ] || echo none")
        .needs(&["sh", "tr", "cat", "rm", "mv", "sort", "awk"])
        .file("configure.sh", CONFIGURE)
        .expect("1\nnone\n", 0, Written)
}

fn script_build() -> CaseResult {
    sh("sh build.sh && cat out/a.o out/b.o")
        .needs(&["sh", "mkdir", "basename", "tr", "cat"])
        .dir("src")
        .file("build.sh", BUILD)
        .file("src/a.c", r#"int a;
"#)
        .file("src/b.c", r#"int b;
"#)
        .expect("CC src/a.c\nCC src/b.c\nbuilt 2\nINT A;\nINT B;\n", 0, Empty)
}

fn script_build_incremental() -> CaseResult {
    sh("sh build.sh >/dev/null && touch -t 200001010000 out/a.o && sh build.sh && sh build.sh")
        .needs(&["sh", "mkdir", "basename", "tr", "touch"])
        .dir("src")
        .file("build.sh", BUILD)
        .file("src/a.c", r#"int a;
"#)
        .file("src/b.c", r#"int b;
"#)
        .expect("CC src/a.c\nbuilt 1\nbuilt 0\n", 0, Empty)
}

fn script_build_fails() -> CaseResult {
    util(&["sh", "build.sh"])
        .needs(&["mkdir", "basename", "tr"])
        .file("build.sh", BUILD)
        .expect("CC src/*.c\n", NONZERO, Written)
}

fn script_rotate() -> CaseResult {
    sh(r#"sh rotate.sh app.log 2 && for f in app.log app.log.1 app.log.2 app.log.3; do if [ -f $f ]; then echo "$f=$(cat $f)"; fi; done"#)
        .needs(&["sh", "rm", "mv", "cat"])
        .file("rotate.sh", ROTATE)
        .file("app.log", r#"v1
"#)
        .expect("rotated app.log\napp.log=\napp.log.1=v1\n", 0, Empty)
}

fn script_rotate_keep() -> CaseResult {
    sh(r#"for v in v1 v2 v3; do echo $v > app.log; sh rotate.sh app.log 2 >/dev/null; done; echo v4 > app.log; for f in app.log app.log.1 app.log.2 app.log.3; do if [ -f $f ]; then echo "$f=$(cat $f)"; fi; done"#)
        .needs(&["sh", "rm", "mv", "cat"])
        .file("rotate.sh", ROTATE)
        .expect("app.log=v4\napp.log.1=v3\napp.log.2=v2\n", 0, Empty)
}

fn script_rotate_missing() -> CaseResult {
    sh("sh rotate.sh nope.log 2>err; echo $?; cat err")
        .needs(&["sh", "rm", "mv", "cat"])
        .file("rotate.sh", ROTATE)
        .expect("1\nrotate: nope.log: no such file\n", 0, Empty)
}

fn script_csv_report() -> CaseResult {
    util(&["sh", "report.sh", "sales.csv"])
        .needs(&["awk", "sort"])
        .file("report.sh", REPORT)
        .file("sales.csv", SALES)
        .expect("region,qty,revenue\neast,5,27.50\nnorth,10,25.00\nwest,1,10.00\ntotal,,62.50\n", 0, Empty)
}

fn script_csv_report_header_only() -> CaseResult {
    util(&["sh", "report.sh", "sales.csv"])
        .needs(&["awk", "sort"])
        .file("report.sh", REPORT)
        .file("sales.csv", r#"region,item,qty,price
"#)
        .expect("region,qty,revenue\ntotal,,0.00\n", 0, Empty)
}

fn script_walk() -> CaseResult {
    util(&["sh", "walk.sh", "t"])
        .dir("t")
        .dir("t/sub")
        .dir("t/sub/deep")
        .file("walk.sh", WALK)
        .file("t/a.txt", "")
        .file("t/sub/b.txt", "")
        .file("t/sub/deep/c.txt", "")
        .file("t/z", "")
        .file("t/.hidden", "")
        .expect("a.txt\nsub/\n  b.txt\n  deep/\n    c.txt\nz\n.hidden\n", 0, Empty)
}

fn script_walk_spaces() -> CaseResult {
    util(&["sh", "walk.sh", "t"])
        .dir("t")
        .dir("t/my dir")
        .file("walk.sh", WALK)
        .file("t/my dir/x y", "")
        .file("t/a b", "")
        .expect("a b\nmy dir/\n  x y\n", 0, Empty)
}

fn script_walk_symlink() -> CaseResult {
    sh("ln -s d t/link && sh walk.sh t")
        .needs(&["ln"])
        .dir("t")
        .dir("t/d")
        .file("walk.sh", WALK)
        .file("t/d/f", "")
        .expect("d/\n  f\nlink\n", 0, Empty)
}

fn script_template() -> CaseResult {
    util(&["sh", "template.sh", "Ada", "tea", "cake"])
        .needs(&["cat"])
        .file("template.sh", TEMPLATE)
        .expect("Dear Ada,\n\nYou ordered 2 item(s):\n  - tea\n  - cake\nTotal due: $10.00\n", 0, Empty)
}

fn script_template_quoted() -> CaseResult {
    util(&["sh", "generate.sh", "ok", "two"])
        .needs(&["cat"])
        .file("generate.sh", GENERATE)
        .expect("generated: ok (2)\n", 0, Empty)
}

fn script_getopts() -> CaseResult {
    util(&["sh", "args.sh", "-v", "-o", "out.txt", "-n3", "x", "y z"])
        .file("args.sh", ARGS)
        .expect("verbose=1 out=out.txt n=3\nargs=2\narg: x\narg: y z\n", 0, Empty)
}

fn script_getopts_grouped() -> CaseResult {
    util(&["sh", "args.sh", "-vn", "2", "--", "-x"])
        .file("args.sh", ARGS)
        .expect("verbose=1 out=- n=2\nargs=1\narg: -x\n", 0, Empty)
}

fn script_getopts_unknown() -> CaseResult {
    sh("sh args.sh -q x 2>err; echo $?; tail -n 1 err")
        .needs(&["sh", "tail"])
        .file("args.sh", ARGS)
        .expect("2\nusage: args.sh [-v] [-o file] [-n count] arg...\n", 0, Empty)
}

fn script_getopts_missing_arg() -> CaseResult {
    sh("sh args.sh -o 2>err; echo $?; tail -n 1 err")
        .needs(&["sh", "tail"])
        .file("args.sh", ARGS)
        .expect("2\nusage: args.sh [-v] [-o file] [-n count] arg...\n", 0, Empty)
}

fn script_trap_cleanup() -> CaseResult {
    sh(r#"sh cleanup.sh; echo $?; for d in work.*; do if [ -e "$d" ]; then echo left; fi; done"#)
        .needs(&["mkdir", "rm", "cat"])
        .file("cleanup.sh", CLEANUP)
        .expect("worked on data\ncleaned\n0\n", 0, Empty)
}

fn script_trap_cleanup_exit() -> CaseResult {
    sh(r#"sh cleanup.sh fail; echo $?; for d in work.*; do if [ -e "$d" ]; then echo left; fi; done"#)
        .needs(&["mkdir", "rm", "cat"])
        .file("cleanup.sh", CLEANUP)
        .expect("failing\ncleaned\n3\n", 0, Empty)
}

fn script_trap_cleanup_int() -> CaseResult {
    sh(r#"sh cleanup.sh int; echo $?; for d in work.*; do if [ -e "$d" ]; then echo left; fi; done"#)
        .needs(&["mkdir", "rm", "cat"])
        .file("cleanup.sh", CLEANUP)
        .expect("interrupted\ncleaned\n130\n", 0, Empty)
}

fn script_source_lib() -> CaseResult {
    util(&["sh", "main.sh", "ab", "cd"])
        .needs(&["dirname", "tr"])
        .dir("lib")
        .file("main.sh", MAIN)
        .file("lib/util.sh", UTIL_LIB)
        .expect("AB\nab,cd\ndone\n", 0, Empty)
}

fn script_source_lib_die() -> CaseResult {
    util(&["sh", "main.sh", "ab"])
        .needs(&["dirname", "tr"])
        .dir("lib")
        .file("main.sh", MAIN)
        .file("lib/util.sh", UTIL_LIB)
        .expect("AB\nab\n", 1, Written)
}

fn script_source_lib_elsewhere() -> CaseResult {
    sh("cd w && sh ../main.sh x y")
        .needs(&["sh", "dirname", "tr"])
        .dir("lib")
        .dir("w")
        .file("main.sh", MAIN)
        .file("lib/util.sh", UTIL_LIB)
        .expect("X\nx,y\ndone\n", 0, Empty)
}

// The scripts the scripts category writes to files and runs.

const WORDFREQ: &str = r#"#!/bin/sh
# wordfreq.sh FILE [N]: the N most frequent words of FILE (- for standard input), most frequent first.
cat "$1" | tr -cs 'A-Za-z' '\n' | tr 'A-Z' 'a-z' | sort | uniq -c | sort -k1,1nr -k2 | head -n "${2:-3}" |
while read count word; do
    printf '%s %s\n' "$word" "$count"
done
"#;

const CONFIGURE: &str = r##"#!/bin/sh
# configure.sh [--prefix=DIR]: probe for tools and write config.h.
prefix=/usr/local
for arg; do
    case $arg in
        --prefix=*) prefix=${arg#--prefix=} ;;
        *) echo "configure: unknown option $arg" >&2; exit 1 ;;
    esac
done
out=config.h
: > "$out.tmp"
for tool in sh sort awk no_such_tool_zz; do
    name=$(echo "$tool" | tr 'a-z' 'A-Z')
    if command -v "$tool" >/dev/null 2>&1; then
        echo "#define HAVE_$name 1" >> "$out.tmp"
    else
        echo "/* #undef HAVE_$name */" >> "$out.tmp"
    fi
done
cat > conftest.c <<EOF
int main(void) { return 0; }
EOF
[ -s conftest.c ] && echo "#define HAVE_CONFTEST 1" >> "$out.tmp"
rm -f conftest.c
echo "#define PREFIX \"$prefix\"" >> "$out.tmp"
mv "$out.tmp" "$out"
echo "configure: wrote $out"
"##;

const BUILD: &str = r#"#!/bin/sh
# build.sh: rebuild out/NAME.o from src/NAME.c whenever the source is newer.
set -e
mkdir -p out
built=0
for src in src/*.c; do
    obj=out/$(basename "$src" .c).o
    if [ ! -f "$obj" ] || [ "$src" -nt "$obj" ]; then
        echo "CC $src"
        tr 'a-z' 'A-Z' < "$src" > "$obj"
        built=$((built + 1))
    fi
done
echo "built $built"
"#;

const ROTATE: &str = r#"#!/bin/sh
# rotate.sh LOG [KEEP]: LOG becomes LOG.1, LOG.1 becomes LOG.2 and so on, keeping KEEP old logs.
log=$1 keep=${2:-3}
[ -f "$log" ] || { echo "rotate: $log: no such file" >&2; exit 1; }
rm -f "$log.$keep"
i=$keep
while [ "$i" -gt 1 ]; do
    prev=$((i - 1))
    if [ -f "$log.$prev" ]; then mv "$log.$prev" "$log.$i"; fi
    i=$prev
done
mv "$log" "$log.1"
: > "$log"
echo "rotated $log"
"#;

const REPORT: &str = r#"#!/bin/sh
# report.sh FILE: quantity and revenue per region, and the total, from a CSV with a header.
echo "region,qty,revenue"
awk -F, 'NR > 1 { q[$1] += $3; r[$1] += $3 * $4 } END { for (k in q) printf "%s,%d,%.2f\n", k, q[k], r[k] }' "$1" | sort
awk -F, 'NR > 1 { t += $3 * $4 } END { printf "total,,%.2f\n", t }' "$1"
"#;

const SALES: &str = r#"region,item,qty,price
east,widget,3,2.50
west,gadget,1,10.00
east,gadget,2,10.00
north,widget,10,2.50
"#;

const WALK: &str = r#"#!/bin/sh
# walk.sh DIR: list a tree depth first, indented, with directories marked by /.
walk() {
    for entry in "$1"/* "$1"/.[!.]*; do
        [ -e "$entry" ] || [ -h "$entry" ] || continue
        name=${entry##*/}
        if [ -d "$entry" ] && [ ! -h "$entry" ]; then
            echo "$2$name/"
            walk "$entry" "$2  "
        else
            echo "$2$name"
        fi
    done
}
walk "${1:-.}" ""
"#;

const TEMPLATE: &str = r#"#!/bin/sh
# template.sh NAME ITEM...: render a letter from a here-document.
name=$1; shift
cat <<EOF
Dear $name,

You ordered $# item(s):
$(for item; do printf '  - %s\n' "$item"; done)
Total due: \$$(( $# * 5 )).00
EOF
"#;

const GENERATE: &str = r#"#!/bin/sh
# generate.sh: write a script from a quoted here-document and run it.
cat > gen.sh <<'EOF'
echo "generated: $1 ($#)"
EOF
sh gen.sh "$@"
"#;

const ARGS: &str = r#"#!/bin/sh
# args.sh [-v] [-o FILE] [-n COUNT] ARG...
verbose=0 out=- n=1
while getopts vo:n: opt; do
    case $opt in
        v) verbose=1 ;;
        o) out=$OPTARG ;;
        n) n=$OPTARG ;;
        \?) echo "usage: args.sh [-v] [-o file] [-n count] arg..." >&2; exit 2 ;;
    esac
done
shift $((OPTIND - 1))
echo "verbose=$verbose out=$out n=$n"
echo "args=$#"
for a; do echo "arg: $a"; done
"#;

const CLEANUP: &str = r#"#!/bin/sh
# cleanup.sh [fail|int]: work in a temporary directory that is removed however the script ends.
tmp=./work.$$
mkdir "$tmp" || exit 1
trap 'rm -rf "$tmp"; echo cleaned' EXIT
trap 'echo interrupted; exit 130' INT
echo data > "$tmp/file"
case $1 in
    fail) echo failing; exit 3 ;;
    int) kill -INT $$ ;;
esac
echo "worked on $(cat "$tmp/file")"
"#;

const UTIL_LIB: &str = r#"# util.sh: helpers that main.sh sources.
LIB_VERSION=2
die() { echo "error: $*" >&2; exit 1; }
upper() { echo "$1" | tr 'a-z' 'A-Z'; }
join() { sep=$1; shift; out=; for x; do out=${out:+$out$sep}$x; done; echo "$out"; }
"#;

const MAIN: &str = r#"#!/bin/sh
# main.sh A B: use the helpers in lib/util.sh next to this script.
here=$(dirname "$0")
. "$here/lib/util.sh"
[ "$LIB_VERSION" -ge 2 ] || die "library too old"
upper "$1"
join , "$@"
[ -n "$2" ] || die "need two arguments"
echo done
"#;

fn now_ms() -> u64 {
    time::now_monotonic().map(|t| (t.as_nanos() / 1_000_000) as u64).unwrap_or(0)
}

/// A directory of the case's own under /tmp, removed with everything in it at the end.
struct Scratch {
    dir: String,
}

impl Scratch {
    fn new() -> Result<Scratch, CaseError> {
        let pid = process::getpid()?.raw();
        let dir = format!("/tmp/utilities-{pid}");
        remove_tree(std::path::Path::new(&dir));
        fs::mkdir(&dir, 0o755).map_err(|e| format!("could not create the scratch directory {dir}: {e}"))?;
        Ok(Scratch { dir })
    }

    fn path(&self, name: &str) -> String {
        format!("{}/{name}", self.dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        remove_tree(std::path::Path::new(&self.dir));
    }
}

/// Remove `path` and, for a directory, everything in it, using only read_dir, unlink
/// and rmdir.
fn remove_tree(path: &std::path::Path) {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    remove_tree(&entry.path());
                }
            }
            let _ = std::fs::remove_dir(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The executable file `name` names on PATH, if there is one.
fn on_path(name: &str) -> Option<String> {
    PATH.split(':').map(|dir| format!("{dir}/{name}")).find(|path| {
        std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false) && fs::access(path, fs::X_OK).is_ok()
    })
}

/// What one case runs: the shell with a command string, or a utility found through PATH,
/// with the files, standard input and utilities it needs.
struct Run {
    /// The program's arguments, the first being the name it runs under.
    argv: Vec<&'static str>,
    /// Run argv by /bin/sh rather than by looking argv[0] up on PATH.
    shell: bool,
    dirs: Vec<&'static str>,
    files: Vec<(&'static str, &'static str, u32)>,
    input: &'static str,
    needs: Vec<&'static str>,
}

/// `sh -c script`, run by /bin/sh.
fn sh(script: &'static str) -> Run {
    Run { argv: vec!["sh", "-c", script], shell: true, dirs: Vec::new(), files: Vec::new(), input: "", needs: Vec::new() }
}

/// A utility found through PATH by `argv[0]`, with `argv` as its arguments.
fn util(argv: &[&'static str]) -> Run {
    Run { argv: argv.to_vec(), shell: false, dirs: Vec::new(), files: Vec::new(), input: "", needs: vec![argv[0]] }
}

impl Run {
    /// Utilities the case uses, each of which must be on PATH.
    fn needs(mut self, names: &[&'static str]) -> Run {
        self.needs.extend_from_slice(names);
        self
    }

    /// A directory, mode 0755, created before any file.
    fn dir(mut self, name: &'static str) -> Run {
        self.dirs.push(name);
        self
    }

    /// A file with mode 0644.
    fn file(self, name: &'static str, text: &'static str) -> Run {
        self.file_mode(name, text, 0o644)
    }

    fn file_mode(mut self, name: &'static str, text: &'static str, mode: u32) -> Run {
        self.files.push((name, text, mode));
        self
    }

    /// Standard input; without it the program reads end of file at once.
    fn input(mut self, text: &'static str) -> Run {
        self.input = text;
        self
    }

    /// Pass when the program exits with `status` (or any non-zero status for NONZERO),
    /// prints exactly `stdout`, and writes to stderr exactly when `stderr` says so.
    fn expect(self, stdout: &str, status: i32, stderr: Stderr) -> CaseResult {
        let missing: Vec<&str> = self.needs.iter().copied().filter(|name| on_path(name).is_none()).collect();
        if self.shell && fs::access(SH, fs::X_OK).is_err() {
            return fail(format!("{SH} is not installed"));
        }
        if !missing.is_empty() {
            return fail(format!("not on PATH {PATH}: {}", missing.join(", ")));
        }
        let path = if self.shell { String::from(SH) } else { on_path(self.argv[0]).unwrap_or_default() };
        let scratch = Scratch::new()?;
        for dir in &self.dirs {
            let path = scratch.path(dir);
            std::fs::create_dir_all(&path).map_err(|e| format!("could not create {path}: {e}"))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("could not set the mode of {path}: {e}"))?;
        }
        for (name, text, mode) in &self.files {
            let path = scratch.path(name);
            if let Some(parent) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("could not create {}: {e}", parent.display()))?;
            }
            std::fs::write(&path, text).map_err(|e| format!("could not write {path}: {e}"))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode))
                .map_err(|e| format!("could not set the mode of {path}: {e}"))?;
        }
        let ran = run(&path, &self.argv, &scratch.dir, self.input.as_bytes())?;
        let status_ok = match status {
            NONZERO => process::wifexited(ran.status) && process::wexitstatus(ran.status) != 0,
            _ => process::wifexited(ran.status) && process::wexitstatus(ran.status) == status,
        };
        if !status_ok {
            let wanted = if status == NONZERO { String::from("a non-zero status") } else { format!("status {status}") };
            return fail(format!("{}, expected {wanted}: {}", ran.ended(), ran.err_text()));
        }
        if ran.stdout != stdout.as_bytes() {
            return fail(format!("stdout {} expected {}", shown(&ran.stdout), shown(stdout.as_bytes())));
        }
        match (stderr, ran.stderr.is_empty()) {
            (Empty, false) => fail(format!("wrote to stderr: {}", ran.err_text())),
            (Written, true) => fail("wrote nothing to stderr, where a diagnostic is required"),
            _ => Ok(()),
        }
    }
}

/// Bytes as a quoted string short enough for a serial line.
fn shown(bytes: &[u8]) -> String {
    let text = format!("{:?}", String::from_utf8_lossy(bytes));
    if text.len() <= 64 {
        return text;
    }
    let mut end = 60;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// What a program the suite started left behind.
struct Ran {
    status: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Ran {
    fn ended(&self) -> String {
        if process::wifexited(self.status) {
            format!("exit status {}", process::wexitstatus(self.status))
        } else if process::wifsignaled(self.status) {
            format!("signal {}", process::wtermsig(self.status))
        } else {
            format!("wait status {:#x}", self.status)
        }
    }

    fn err_text(&self) -> String {
        let text = String::from_utf8_lossy(&self.stderr);
        let text = text.trim();
        if text.is_empty() { String::from("nothing on stderr") } else { format!("stderr {:?}", text) }
    }
}

fn cstrings(items: &[&str]) -> (Vec<CString>, Vec<*const u8>) {
    let owned: Vec<CString> = items.iter().map(|s| CString::new(*s).unwrap_or_default()).collect();
    let mut ptrs: Vec<*const u8> = owned.iter().map(|c| c.as_ptr() as *const u8).collect();
    ptrs.push(core::ptr::null());
    (owned, ptrs)
}

/// Close-on-exec pipe ends.
fn pipe() -> Result<(Fd, Fd), CaseError> {
    io::pipe2(status_flags::O_CLOEXEC).map_err(|e| format!("pipe failed: {e}").into())
}

fn nonblocking(fd: Fd) {
    if let Ok(flags) = io::fcntl(fd, fcntl_cmd::F_GETFL, 0) {
        let _ = io::fcntl(fd, fcntl_cmd::F_SETFL, flags | status_flags::O_NONBLOCK as i64);
    }
}

/// Run `path` with `argv` in `dir` under umask 022 with default signal actions, feeding it
/// `input` and collecting its stdout and stderr until it exits, or killing it 1.5 seconds
/// before the case's limit.
fn run(path: &str, argv: &[&str], dir: &str, input: &[u8]) -> Result<Ran, CaseError> {
    let (in_r, in_w) = pipe()?;
    let (out_r, out_w) = pipe()?;
    let (err_r, err_w) = pipe()?;
    let (exec_r, exec_w) = pipe()?;
    let (_argv_owned, argv_ptrs) = cstrings(argv);
    let (_env_owned, env_ptrs) = cstrings(ENV);
    let cpath = CString::new(path).map_err(|_| "program path holds a NUL")?;
    let cdir = CString::new(dir).map_err(|_| "directory path holds a NUL")?;

    let child = match process::fork() {
        Ok(process::ForkResult::Child) => {
            let why = (|| -> Result<(), String> {
                process::chdir(cdir.as_bytes_with_nul()).map_err(|e| format!("chdir {dir} failed: {e}"))?;
                // SAFETY: umask takes no pointers.
                unsafe { raw::syscall1(nr::UMASK, 0o022) };
                // The program starts with every signal at its default action and none
                // blocked, as it would from init: this suite is a Rust program, which
                // ignores SIGPIPE, and exec keeps ignored signals ignored.
                let default = signal::Sigaction::default();
                for sig in 1..=64 {
                    let _ = signal::sigaction(sig, Some(&default), None);
                }
                let _ = signal::sigprocmask(signal::SIG_SETMASK, Some(&0), None);
                io::dup2(in_r, Fd::STDIN).map_err(|e| format!("dup2 failed: {e}"))?;
                io::dup2(out_w, Fd::STDOUT).map_err(|e| format!("dup2 failed: {e}"))?;
                io::dup2(err_w, Fd::STDERR).map_err(|e| format!("dup2 failed: {e}"))?;
                match process::execve(cpath.as_bytes_with_nul(), argv_ptrs.as_ptr(), env_ptrs.as_ptr()) {
                    Ok(never) => match never {},
                    Err(e) => Err(format!("execve {path} failed: {e}")),
                }
            })();
            if let Err(why) = why {
                let _ = io::write(exec_w, why.as_bytes());
            }
            process::exit(127);
        }
        Ok(process::ForkResult::Parent(pid)) => pid.raw() as i32,
        Err(e) => {
            for fd in [in_r, in_w, out_r, out_w, err_r, err_w, exec_r, exec_w] {
                let _ = io::close(fd);
            }
            return Err(format!("fork failed: {e}").into());
        }
    };
    for fd in [in_r, out_w, err_w, exec_w] {
        let _ = io::close(fd);
    }
    for fd in [in_w, out_r, err_r, exec_r] {
        nonblocking(fd);
    }

    let allowed = case_ms_left().saturating_sub(RESERVE_MS);
    let deadline = now_ms() + allowed;
    let mut readers = [(out_r, Vec::new()), (err_r, Vec::new()), (exec_r, Vec::new())];
    let mut open = [true; 3];
    let mut writer = if input.is_empty() { let _ = io::close(in_w); None } else { Some(in_w) };
    let mut sent = 0;
    let mut status = None;
    let mut exited_at = 0;
    loop {
        let mut fds: Vec<PollFd> = Vec::new();
        for (i, (fd, _)) in readers.iter().enumerate() {
            if open[i] {
                fds.push(PollFd::new(*fd, poll_events::POLLIN));
            }
        }
        if let Some(w) = writer {
            fds.push(PollFd::new(w, poll_events::POLLOUT));
        }
        if !fds.is_empty() {
            let _ = io::poll(&mut fds, 10);
        }
        for (i, (fd, buf)) in readers.iter_mut().enumerate() {
            if !open[i] {
                continue;
            }
            let mut chunk = [0u8; 4096];
            loop {
                match io::read(*fd, &mut chunk) {
                    Ok(0) => {
                        open[i] = false;
                        let _ = io::close(*fd);
                        break;
                    }
                    Ok(n) => {
                        if buf.len() < 1 << 20 {
                            buf.extend_from_slice(&chunk[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        if let Some(w) = writer {
            match io::write(w, &input[sent..]) {
                Ok(n) => sent += n,
                Err(Error::Os(Errno::EAGAIN)) => {}
                Err(_) => sent = input.len(),
            }
            if sent >= input.len() {
                let _ = io::close(w);
                writer = None;
            }
        }
        if status.is_none() {
            let mut s = 0;
            if let Ok(pid) = process::waitpid(child, &mut s, process::WNOHANG) {
                if pid.raw() as i32 == child {
                    status = Some(s);
                    exited_at = now_ms();
                }
            }
        }
        if let Some(s) = status {
            // Done once every pipe is closed; a descendant still holding one gets 200 ms.
            if !open.iter().any(|o| *o) || now_ms() > exited_at + 200 {
                for (i, (fd, _)) in readers.iter().enumerate() {
                    if open[i] {
                        let _ = io::close(*fd);
                    }
                }
                if let Some(w) = writer {
                    let _ = io::close(w);
                }
                let [(_, stdout), (_, stderr), (_, exec_err)] = readers;
                if !exec_err.is_empty() {
                    return Err(String::from_utf8_lossy(&exec_err).into_owned().into());
                }
                return Ok(Ran { status: s, stdout, stderr });
            }
        } else if now_ms() > deadline {
            let _ = signal::kill(child, signal::SIGKILL);
            let mut s = 0;
            let _ = process::waitpid(child, &mut s, 0);
            for (i, (fd, _)) in readers.iter().enumerate() {
                if open[i] {
                    let _ = io::close(*fd);
                }
            }
            if let Some(w) = writer {
                let _ = io::close(w);
            }
            return Err(format!("{path} did not finish within {allowed} ms and was killed").into());
        }
    }
}

fn main() { SUITE.run() }
