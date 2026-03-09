import re
with open('metanode/meta-consensus/core/src/lib.rs', 'r') as f:
    lib = f.read()

if 'pub mod core;' not in lib and 'pub mod proposer;' not in lib:
    # Need to find how core is exported
    pass

with open('metanode/meta-consensus/core/src/core/mod.rs', 'w') as f:
    f.write('pub mod proposer;\n')

