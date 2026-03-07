import re

def extract_methods():
    with open('metanode/meta-consensus/core/src/core.rs', 'r') as f:
        content = f.read()
    
    methods_to_extract = [
        "add_blocks",
        "check_block_refs",
        "get_missing_blocks"
    ]
    
    extracted = ""
    for method in methods_to_extract:
        start_idx = content.find(f"fn {method}(")
        if start_idx == -1:
            start_idx = content.find(f"    fn {method}(")
            if start_idx == -1:
                start_idx = content.find(f"pub(crate) fn {method}(")
                if start_idx == -1:
                    start_idx = content.find(f"    pub(crate) fn {method}(")
        
        if start_idx == -1:
            print(f"Match not found for {method}")
            continue
            
        lines = content[:start_idx].split('\n')
        comments_start = start_idx
        for i in range(len(lines) - 2, -1, -1):
            if lines[i].strip().startswith('///') or lines[i].strip().startswith('#['):
                comments_start = content.rfind(lines[i], 0, comments_start)
            else:
                break
                
        brace_idx = content.find('{', start_idx)
        if brace_idx == -1:
            continue
            
        count = 1
        curr_idx = brace_idx + 1
        while count > 0 and curr_idx < len(content):
            if content[curr_idx] == '{':
                count += 1
            elif content[curr_idx] == '}':
                count -= 1
            curr_idx += 1
            
        end_idx = curr_idx
        method_str = content[comments_start:end_idx]
        
        # Change visibility to pub(crate) if it's not already
        if "pub(crate) fn" not in method_str:
            method_str = method_str.replace(f"fn {method}", f"pub(crate) fn {method}", 1)
            
        extracted += method_str + "\n\n"
        
        # Remove from core.rs
        content = content[:comments_start] + content[end_idx:]

    # Add pub mod block_importer; in core.rs
    imports_end = content.find("pub mod proposer;") + 17
    if "pub mod block_importer;" not in content:
        content = content[:imports_end] + "\npub mod block_importer;\n" + content[imports_end:]

    with open('metanode/meta-consensus/core/src/core/block_importer.rs', 'w') as f:
        f.write(f"""use std::collections::BTreeSet;
use tracing::debug;

use consensus_types::block::BlockRef;

use crate::{{
    block::VerifiedBlock,
    core::Core,
    error::ConsensusResult,
}};

impl Core {{
{extracted}
}}
""")
        
    with open('metanode/meta-consensus/core/src/core.rs', 'w') as f:
        f.write(content)

extract_methods()
