# nmb.py CALLGRIND.OUT: macroblocks decoded, as the calls into the hot-path instance of `start_mb` (one a macroblock).
import sys, re
names = {}; cfn = ''; n = 0
for line in open(sys.argv[1]):
    if line.startswith('cfn='):
        m = re.match(r'cfn=\((\d+)\)(?: (.*))?', line.strip())
        if m.group(2): names[m.group(1)] = m.group(2)
        cfn = names.get(m.group(1), '')
    elif line.startswith('fn='):
        m = re.match(r'fn=\((\d+)\)(?: (.*))?', line.strip())
        if m and m.group(2): names[m.group(1)] = m.group(2)
    elif line.startswith('calls=') and 'enter' in cfn and 'start_mb<' in cfn:
        n += int(line[6:].split()[0])
print(n)
