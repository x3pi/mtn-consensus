import re

with open('metanode/meta-consensus/core/src/core_thread.rs', 'r') as f:
    content = f.read()

# Add missing methods to CoreThreadDispatcher trait
if "fn set_propagation_delay(" not in content:
    content = content.replace(
        "pub trait CoreThreadDispatcher: Sync + Send + 'static {",
        "pub trait CoreThreadDispatcher: Sync + Send + 'static {\n    fn set_propagation_delay(&mut self, round: consensus_types::block::Round);\n    fn set_last_known_proposed_round(&mut self, round: consensus_types::block::Round);\n"
    )

with open('metanode/meta-consensus/core/src/core_thread.rs', 'w') as f:
    f.write(content)
