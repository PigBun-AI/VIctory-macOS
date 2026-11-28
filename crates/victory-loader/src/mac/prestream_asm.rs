/// Authored assembly shared by the runtime and the offline COFF fixture.
macro_rules! l8_source {
    () => {
        r#"
// L8, Win64 x86-64. Copied as one position-independent block BEFORE any title thread.
// No library call, TLS, SIMD, allocation, logging or spin on a title thread.
.intel_syntax noprefix
.text
.globl l8_begin, l8_end
l8_begin:
.Lstart:
.Ltable: .quad 0
.Lquery: .quad 0
.Lresult: .quad 0
.Lgrade: .quad 0
.Lphase: .quad 0
.Lremove: .quad 0
.Lbase: .quad 0
.Lloss: .quad 0
.Llife: .quad 0
.macro at n
.space \n-(.-.Lstart), 0x90
.endm
.macro save
 pushfq
 push rax
 push rcx
 push rdx
 push r8
 push r9
 push r10
 push r11
.endm
.macro restore
 pop r11
 pop r10
 pop r9
 pop r8
 pop rdx
 pop rcx
 pop rax
 popfq
.endm
at 0x80
// Original CALL here preserves the callee's return PC and its shadow space. The
// 2-argument callee has no stack arguments. Caller frame [rsp+20h..23h] is
// padding next to the actual key at +24h (callee-entry rsp is 8 lower).
 mov [rsp+0x28],ecx
 jmp qword ptr [rip+.Lquery]
at 0x100
 save
 mov r8d,[rsp+64+0x20]
 shl r8,32
 mov edx,[rsp+64+0x24]
 or r8,rdx
 call .Lslot
 test r10,r10
 jz 2f
 mov rax,[rsp+48]
 test al,al
 setne al
 movzx eax,al
 lock inc qword ptr [r10+8+rax*8]
2: restore
 xor al,1
 movzx eax,al
 jmp qword ptr [rip+.Lresult]
at 0x200
// This function tests signed grade only. Cursor equality and ready(+e) are
// diagnostic context, NOT additional predicates at this site. Only inspect
// caller locals for the six verified generic-loop return PCs (NOT type 1).
 push r11
 push r10
 mov r11,rax
 mov eax,[r11+8]
 mov r10d,[rsi+12]
 cmp eax,r10d
 jge 5f
 save // preserve original CMP flags and both sampled operands
 mov rdx,[rsp+80+0x28]
 sub rdx,[rip+.Lbase]
 cmp rdx,0x227ca9c
 ja 4f
 lea rcx,[rip+.Lreturns]
 mov r9d,6
1: cmp edx,[rcx+r9*4-4]
 je 2f
 dec r9d
 jnz 1b
 jmp 4f
2: mov rcx,[rsp+80+0x30+0x48]
 mov r8d,[rcx+0x1c]
 shl r8,32
 mov rcx,[rsp+80+0x30+0x30]
 mov ecx,[rcx+4]
 or r8,rcx
 push r9
 call .Lslot
 pop r9
 test r10,r10
 jz 4f
 xor eax,eax
 mov ecx,-1
 lock cmpxchg [r10+24],ecx
 jne 4f
 mov rax,[r11]
 mov [r10+32],rax
 mov eax,[rsp+48]
 mov [r10+40],eax
 mov eax,[rsp+8]
 mov [r10+44],eax
 movzx eax,byte ptr [r11+14]
 mov [r10+48],eax
 mov [r10+56],r15
 mov [r10+24],r9d
4: restore
5: pop r10
 pop r11
 jmp qword ptr [rip+.Lgrade]
.Lreturns: .long 0x227c6f8,0x227c79a,0x227c860,0x227c90e,0x227c9c8,0x227ca9c
at 0x300
 save
 mov rcx,rax
 mov r10,[rip+.Llife]
 movzx r9d,bl
 call .Lrecord
 restore
 mov [rax+0x28],bl
 jmp qword ptr [rip+.Lphase]
at 0x380
 save
 mov r10,[rip+.Llife]
 add r10,32
 mov r9d,255
 call .Lrecord
 restore
 push rsi
 push rdi
 sub rsp,0x28
 jmp qword ptr [rip+.Lremove]
// Bounded seqlock: never wait in a title thread. tsc is raw monotonic ticks;
// print alongside a paired host elapsed-time/TSC sample, not title milliseconds.
.Lrecord:
 mov rax,[r10]
 test al,1
 jnz .Llost
 lea r8,[rax+1]
 lock cmpxchg [r10],r8
 jne .Llost
 mov eax,[rcx+0x1c]
 mov [r10+8],eax
 mov al,[rcx+0x28]
 mov [r10+12],al
 mov [r10+13],r9b
 mov al,[rcx+0x51]
 mov [r10+14],al
 rdtsc
 shl rdx,32
 or rax,rdx
 mov [r10+16],rax
 inc r8
 mov [r10],r8
 ret
// In: packed id/key R8. Out R10=slot or zero. Preserve R11. All-zero pair
// has a dedicated slot. Nonzero identities are published with one CAS.
.Lslot:
 mov r10,[rip+.Ltable]
 test r8,r8
 jnz 1f
 add r10,4096*64
 ret
1: mov rax,r8
 shr rax,32
 xor eax,r8d
 imul eax,eax,0x9e3779b1
 shr eax,20
 mov ecx,eax
 mov r9d,32
2: mov edx,ecx
 shl edx,6
 add rdx,r10
 mov rax,[rdx]
 cmp rax,r8
 je 3f
 test rax,rax
 jnz 4f
 lock cmpxchg [rdx],r8
 je 3f
 cmp rax,r8
 je 3f
4: inc ecx
 and ecx,4095
 dec r9d
 jnz 2b
 xor r10d,r10d
.Llost:
 mov rdx,[rip+.Lloss]
 lock inc qword ptr [rdx]
 ret
3: mov r10,rdx
 ret
l8_end:
"#
    };
}
