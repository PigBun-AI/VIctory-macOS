// Shared verbatim by LLVM in the loader and offline COFF disassembler.
macro_rules! l9_source {
    () => {
        r#"
.intel_syntax noprefix
.text
.globl l9_begin, l9_end
l9_begin:
.L9start:
.L9base: .quad 0
.L9global: .quad 0
.L9slots: .quad 0
.L9hist: .quad 0
.L9life: .quad 0
.L9stats: .quad 0
.L9ra: .quad 0
.L9rb: .quad 0
.L9rf: .quad 0
.L9size: .quad 0
.macro l9save
 pushfq
 push rax
 push rcx
 push rdx
 push r8
 push r9
 push r10
 push r11
.endm
.macro l9restore
 pop r11
 pop r10
 pop r9
 pop r8
 pop rdx
 pop rcx
 pop rax
 popfq
.endm
.space 0x80-(.-.L9start),0x90
 call .L9alloc
 mov rax,rdi
 add rsp,0x28
 jmp qword ptr [rip+.L9ra]
.space 0xa0-(.-.L9start),0x90
 call .L9alloc
 mov rax,rdi
 add rsp,0x28
 jmp qword ptr [rip+.L9rb]
.space 0xc0-(.-.L9start),0x90
 call .L9free
 mov r10,[rcx+0x28]
 mov rdx,rax
 jmp qword ptr [rip+.L9rf]
// Return epilogues: RDI is the actual returned node. Reject null, OBJECT,
// mismatched capacity/stride, and non-slot-aligned pointers. No title writes.
.L9alloc:
 l9save
 test rdi,rdi
 jz .L9done
 mov rcx,[rip+.L9global]
 mov rcx,[rcx]
 test rcx,rcx
 jz .L9done
 cmp dword ptr [rcx+12],22500
 jne .L9done
 cmp dword ptr [rcx+8],224
 jne .L9done
 mov rax,rdi
 sub rax,[rcx]
 cmp rax,22500*224
 jae .L9done
 xor edx,edx
 mov ecx,224
 div ecx
 test edx,edx
 jnz .L9done
 shl rax,5
 mov r11,[rip+.L9slots]
 add r11,rax
 call .L9caller
 mov r10,r8
 call .L9bucket
 test rcx,rcx
 jz 1f
 lock inc qword ptr [rcx+8]
1: mov rcx,[rip+.L9life]
 mov r8,[rcx]
 mov r9,0x100000000
 test r8,r8
 jz 2f
 test r8b,1
 jnz 2f
 mov edx,[rcx+8]
 cmp r8,[rcx]
 cmove r9,rdx
2: inc qword ptr [r11]
 mov r8,[r11+8]
 mov [r11+8],r10
 mov [r11+16],r9
 rdtsc
 shl rdx,32
 or rax,rdx
 mov [r11+24],rax
 inc qword ptr [r11]
 mov rcx,[rip+.L9stats]
 test r8,r8
 jnz .L9overwrite
 mov eax,1
 lock xadd [rcx],rax
 inc eax
 // Exact first crossing of each observed-owner occupancy power. A reporter
 // thread drains the bitmask; its snapshot is labelled deferred, not historical.
 lea edx,[rax-1]
 test eax,edx
 jnz .L9done
 bsf eax,eax
 lock bts [rcx+8],rax
 jmp .L9done
.L9overwrite:
 lock inc qword ptr [rcx+24]
 jmp .L9done
// Here the destructor has chosen delete, computed an index, but has NOT
// published the node to other threads. Never clear a later reuse.
.L9free:
 l9save
 cmp rax,22500
 jae .L9done
 cmp dword ptr [rcx+12],22500
 jne .L9done
 cmp dword ptr [rcx+8],224
 jne .L9done
 shl rax,5
 mov r11,[rip+.L9slots]
 add r11,rax
 call .L9caller
 call .L9bucket
 test rcx,rcx
 jz 3f
 lock inc qword ptr [rcx+16]
3: mov r8,[r11+8]
 test r8,r8
 jz .L9miss
 call .L9bucket
 test rcx,rcx
 jz 4f
 lock inc qword ptr [rcx+24]
4: inc qword ptr [r11]
 mov qword ptr [r11+8],0
 inc qword ptr [r11]
 mov rcx,[rip+.L9stats]
 lock dec qword ptr [rcx]
 jmp .L9done
.L9miss:
 mov rcx,[rip+.L9stats]
 lock inc qword ptr [rcx+32]
.L9done:
 l9restore
 ret
// Two private return PCs, eight saved registers/flags, 0x38 title frame.
.L9caller:
 mov r8,[rsp+16+64+0x38]
 sub r8,[rip+.L9base]
 cmp r8,[rip+.L9size]
 jb 5f
 mov r8,0x100000000
5: inc r8
 ret
// Fixed open-addressed table, key=RVA+1. At most 32 probes, report losses.
// Preserves r8 (key), r10 (owner), r11 (slot).
.L9bucket:
 imul edx,r8d,0x9e3779b1
 shr edx,20
 mov r9d,32
6: mov ecx,edx
 shl ecx,5
 add rcx,[rip+.L9hist]
 mov rax,[rcx]
 cmp rax,r8
 je 7f
 test rax,rax
 jnz 8f
 lock cmpxchg [rcx],r8
 je 7f
 cmp rax,r8
 je 7f
8: inc edx
 and edx,4095
 dec r9d
 jnz 6b
 mov rcx,[rip+.L9stats]
 lock inc qword ptr [rcx+16]
 xor ecx,ecx
7: ret
l9_end:
"#
    };
}
