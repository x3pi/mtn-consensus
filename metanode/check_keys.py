
import json
import base64
import sys

# Mock verification or simple string check if we can't do crypto
# Assuming Ed25519 or BLS.
# If BLS, length is different. 
# Protocol Key is typically Ed25519 in mtn-consensus? 32 bytes pub, 64 priv.

def check_keys():
    committee_file = 'config/committee.json'
    with open(committee_file) as f:
        committee = json.load(f)

    for i in range(4):
        priv_file = f'config/node_{i}_protocol_key.json'
        with open(priv_file) as f:
            priv_b64 = f.read().strip()
        
        # In committee
        pub_b64 = committee['authorities'][i]['protocol_key']

        print(f"Node {i}:")
        print(f"  Priv (b64 len={len(priv_b64)}): {priv_b64[:10]}...")
        print(f"  Pub  (b64 len={len(pub_b64)}): {pub_b64[:10]}...")
        
        # If we can't verify crypto, just check lengths/existence
        # Real verification requires Ed25519 lib.
        # But wait, if priv is 88 chars (64 bytes), and pub is 44 chars (32 bytes).
        # We can't verify easily without library.
        # But we can assume if they look like b64, it's likely fine IF generated together.
        pass

if __name__ == '__main__':
    check_keys()
