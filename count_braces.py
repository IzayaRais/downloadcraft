import sys

lines = open('src/app.rs', encoding='utf-8').read().split('\n')
depth_c = depth_p = 0
for i, ln in enumerate(lines[1975:2210], start=1976):
    s = ln
    code = ''
    k = 0
    q = None
    while k < len(s):
        c = s[k]
        if q:
            if c == '\\':
                k += 1
            elif c == q:
                q = None
        elif c == '"' or c == "'":
            # skip Rust lifetime ticks like 'a
            nxt = s[k + 1] if k + 1 < len(s) else ''
            prv = s[k - 1] if k > 0 else ''
            if c == "'" and (nxt.isalpha() or nxt == '_') and not (prv.isalnum() or prv == '_'):
                pass
            else:
                q = c
        elif c == '/' and k + 1 < len(s) and s[k + 1] == '/':
            break
        else:
            code += c
        k += 1
    for c in code:
        if c == '{':
            depth_c += 1
        elif c == '}':
            depth_c -= 1
        elif c == '(':
            depth_p += 1
        elif c == ')':
            depth_p -= 1
    if depth_c < 0 or depth_p < 0:
        print('NEG at line', i, repr(ln[:100]))
        break
print('end depth curly=', depth_c, 'paren=', depth_p)
