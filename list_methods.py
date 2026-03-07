import re

with open('metanode/meta-consensus/core/src/core.rs', 'r') as f:
    content = f.read()

impl_start = content.find("impl Core {")
impl_end = content.rfind("}")

if impl_start != -1 and impl_end != -1:
    impl_body = content[impl_start:impl_end]
    # find all fn names
    matches = re.findall(r'(?:pub\(crate\)\s+)?fn\s+([a-zA-Z0-9_]+)\s*\(', impl_body)
    for match in matches:
        print(match)
