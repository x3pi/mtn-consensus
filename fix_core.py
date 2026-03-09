with open('metanode/meta-consensus/core/src/core.rs', 'r') as f:
    content = f.read()

# Remove the dangling pub(crate) that was left behind
content = content.replace("    \n    pub(crate) \n\n", "")
content = content.replace("    pub(crate) \n", "")

# Remove the extra `}` at the end of the file. 
# Find the last `}` and the second to last `}`
lines = content.split('\n')
for i in range(len(lines) - 1, -1, -1):
    if lines[i].strip() == '}':
        # Let's count the braces to see if we have an extra one.
        open_b = content.count('{')
        close_b = content.count('}')
        if close_b > open_b:
            lines[i] = ''
        break

with open('metanode/meta-consensus/core/src/core.rs', 'w') as f:
    f.write('\n'.join(lines))
