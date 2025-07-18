; Fork test program for Breenix OS
; Tests the fork() system call from userspace

global _start

section .text

_start:
    ; Print "Before fork\n"
    mov     rax, 1              ; sys_write
    mov     rdi, 1              ; stdout
    mov     rsi, msg_before     ; message
    mov     rdx, msg_before_len ; length
    int     0x80
    
    ; Call fork()
    mov     rax, 5              ; sys_fork
    int     0x80
    
    ; Check if we're parent or child
    test    rax, rax
    jz      child_process       ; rax == 0 means child
    js      fork_error          ; rax < 0 means error
    
parent_process:
    ; Save child PID
    mov     rbx, rax
    
    ; Print "Parent: child PID = "
    mov     rax, 1              ; sys_write
    mov     rdi, 1              ; stdout
    mov     rsi, msg_parent     ; message
    mov     rdx, msg_parent_len ; length
    int     0x80
    
    ; Convert child PID to string and print it (simplified - just print low digit)
    mov     rax, rbx            ; child PID
    and     rax, 0xF            ; Get low nibble
    add     rax, '0'            ; Convert to ASCII
    cmp     rax, '9'
    jle     .print_digit
    add     rax, 7              ; Adjust for hex digits A-F
.print_digit:
    mov     [pid_digit], al
    mov     rax, 1              ; sys_write
    mov     rdi, 1              ; stdout
    mov     rsi, pid_digit      ; single digit
    mov     rdx, 1              ; length
    int     0x80
    
    ; Print newline
    mov     rax, 1              ; sys_write
    mov     rdi, 1              ; stdout
    mov     rsi, newline        ; newline
    mov     rdx, 1              ; length
    int     0x80
    
    ; Parent exits with code 0
    mov     rax, 0              ; sys_exit
    mov     rdi, 0              ; exit code
    int     0x80
    
child_process:
    ; Print "Child: I am the child!\n"
    mov     rax, 1              ; sys_write
    mov     rdi, 1              ; stdout
    mov     rsi, msg_child      ; message
    mov     rdx, msg_child_len  ; length
    int     0x80
    
    ; Child exits with code 42
    mov     rax, 0              ; sys_exit
    mov     rdi, 42             ; exit code
    int     0x80
    
fork_error:
    ; Print "Fork failed!\n"
    mov     rax, 1              ; sys_write
    mov     rdi, 1              ; stdout
    mov     rsi, msg_error      ; message
    mov     rdx, msg_error_len  ; length
    int     0x80
    
    ; Exit with error code
    mov     rax, 0              ; sys_exit
    mov     rdi, 1              ; exit code
    int     0x80

section .data
    msg_before:     db "Before fork", 10
    msg_before_len: equ $ - msg_before
    
    msg_parent:     db "Parent: child PID = "
    msg_parent_len: equ $ - msg_parent
    
    msg_child:      db "Child: I am the child!", 10
    msg_child_len:  equ $ - msg_child
    
    msg_error:      db "Fork failed!", 10
    msg_error_len:  equ $ - msg_error
    
    newline:        db 10

section .bss
    pid_digit:      resb 1