import re

with open('metanode/meta-consensus/core/src/core/proposer.rs', 'r') as f:
    orig = f.read()

impl_start = orig.find('impl Core {}')
if impl_start != -1:
    orig = orig[:impl_start] + "impl Core {\n" + orig[impl_start + 12:] + "\n}\n"

with open('metanode/meta-consensus/core/src/core/proposer.rs', 'w') as f:
    f.write(orig)
