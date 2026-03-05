#!/usr/bin/env python3
"""
Script để sync committee từ Rust committee.json vào Go genesis.json
- Đọc committee từ committee_node_0.json (hoặc committee.json)
- Update genesis.json với validators từ committee
- GIỮ NGUYÊN delegator_stakes và total_staked_amount hiện tại
- Chỉ update keys: authority_key, protocol_key, network_key
"""

import json
import sys
import os
from typing import Dict, List, Any

def load_committee(committee_path: str) -> Dict[str, Any]:
    """Load committee từ Rust committee file"""
    with open(committee_path, 'r') as f:
        return json.load(f)

def load_genesis(genesis_path: str) -> Dict[str, Any]:
    """Load genesis.json từ Go project"""
    with open(genesis_path, 'r') as f:
        return json.load(f)

def save_genesis_backup_first(genesis_path: str, genesis: Dict[str, Any]):
    """Save genesis.json - AN TOÀN: backup trước, sau đó ghi đúng"""
    import os

    # Tạo backup
    backup_path = genesis_path + '.backup_safe'
    if os.path.exists(genesis_path):
        with open(genesis_path, 'r') as f:
            original_content = f.read()
        with open(backup_path, 'w') as f:
            f.write(original_content)
        print(f"   💾 Đã backup file gốc vào: {backup_path}")

    # Ghi file mới với format đẹp
    with open(genesis_path, 'w') as f:
        json.dump(genesis, f, indent=2)

    print(f"   ✅ Đã ghi file mới với {len(genesis.get('validators', []))} validators")

def save_genesis(genesis_path: str, genesis: Dict[str, Any]):
    """Save genesis.json - FULL SAVE (có thể ghi đè)"""
    with open(genesis_path, 'w') as f:
        json.dump(genesis, f, indent=2)

def extract_committee_data(committee: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Extract committee data - hỗ trợ cả CommitteeConfig và Committee format"""
    # Check if it's CommitteeConfig format (có committee field)
    if 'committee' in committee:
        authorities = committee['committee'].get('authorities', [])
    else:
        # Plain Committee format
        authorities = committee.get('authorities', [])
    
    return authorities

def sync_committee_to_genesis(committee_path: str, genesis_path: str):
    """COPY TOÀN BỘ FILE GENESIS RỒI CHỈ SỬA DANH SÁCH VALIDATORS"""
    print(f"📝 Loading committee from: {committee_path}")
    committee = load_committee(committee_path)

    print(f"📝 Loading genesis.json from: {genesis_path}")
    genesis = load_genesis(genesis_path)

    # Extract authorities from committee
    authorities = extract_committee_data(committee)
    print(f"✅ Found {len(authorities)} validators in committee")

    if not authorities:
        print("❌ Error: No validators found in committee!")
        sys.exit(1)

    # COPY TOÀN BỘ genesis object để đảm bảo không mất gì
    final_genesis = genesis.copy()
    original_validators = genesis.get('validators', [])
    print(f"📝 Found {len(original_validators)} existing validators in genesis")
    print(f"📝 Found {len(final_genesis.get('alloc', []))} alloc entries")

    # Tạo danh sách validators mới từ committee nhưng GIỮ NGUYÊN alloc từ genesis gốc
    new_validators = []
    for idx, authority in enumerate(authorities):
        if idx >= len(original_validators):
            print(f"⚠️  Warning: Committee has more validators ({len(authorities)}) than genesis ({len(original_validators)})")
            break

        # COPY TOÀN BỘ validator từ genesis gốc (giữ delegator_stakes, total_staked_amount, etc.)
        validator_copy = original_validators[idx].copy()

        # CHỈ UPDATE cryptographic keys từ committee
        authority_key = authority.get('authority_key', '')
        protocol_key = authority.get('protocol_key', '')
        network_key = authority.get('network_key', '')
        hostname = authority.get('hostname', f'node-{idx}')

        validator_copy['authority_key'] = authority_key
        validator_copy['protocol_key'] = protocol_key
        validator_copy['network_key'] = network_key
        validator_copy['hostname'] = hostname
        validator_copy['description'] = f"Validator {hostname} from committee"

        new_validators.append(validator_copy)
        print(f"  ✅ Updated validator {idx}: {hostname} (copied original alloc)")

    # CHỈ THAY THẾ phần validators, giữ nguyên mọi thứ khác
    final_genesis['validators'] = new_validators

    # TÍNH LẠI total_stake, quorum_threshold, validity_threshold từ delegator_stakes
    total_stake = 0
    for validator in new_validators:
        # Lấy total_staked_amount trực tiếp từ genesis (đã tính sẵn)
        total_staked_str = validator.get('total_staked_amount', '0')
        try:
            total_staked_wei = int(total_staked_str)
            # Chia cho 1e18 để chuyển từ wei sang token
            validator_stake = total_staked_wei // (10**18)
            total_stake += validator_stake
            print(f"  📊 Validator stake: {validator_stake} (from {total_staked_str})")
        except ValueError:
            print(f"  ⚠️  Invalid total_staked_amount: {total_staked_str}")

    # Tính threshold theo công thức
    if total_stake > 0:
        quorum_threshold = (total_stake * 2) // 3  # 2/3 total_stake
        validity_threshold = total_stake // 3      # 1/3 total_stake

        final_genesis['total_stake'] = total_stake
        final_genesis['quorum_threshold'] = quorum_threshold
        final_genesis['validity_threshold'] = validity_threshold

        print(f"  📊 Total stake: {total_stake}")
        print(f"  📊 Quorum threshold: {quorum_threshold} (2/3)")
        print(f"  📊 Validity threshold: {validity_threshold} (1/3)")
    else:
        print(f"  ⚠️  No stake found, keeping existing values")

    # CRITICAL: Update epoch_timestamp_ms với current time nếu chưa có hoặc quá cũ
    # Điều này đảm bảo epoch duration được tính từ thời điểm hiện tại
    import time
    current_timestamp_ms = int(time.time() * 1000)
    
    if 'config' not in final_genesis:
        final_genesis['config'] = {}
    
    existing_timestamp = final_genesis['config'].get('epoch_timestamp_ms')
    # Get epoch_duration from config, default to 600s (10 minutes)
    epoch_duration_seconds = final_genesis['config'].get('epoch_duration_seconds', 600)
    
    if existing_timestamp is None:
        # Chưa có timestamp - set current time
        final_genesis['config']['epoch_timestamp_ms'] = current_timestamp_ms
        print(f"  📅 Set epoch_timestamp_ms: {current_timestamp_ms} (was not set)")
    else:
        # Có timestamp - check nếu quá cũ (hơn epoch_duration)
        elapsed_seconds = (current_timestamp_ms - existing_timestamp) / 1000
        if elapsed_seconds > epoch_duration_seconds:  # Hơn epoch_duration -> reset để tránh instant epoch advance
            print(f"  ⚠️  Existing epoch_timestamp_ms is {elapsed_seconds:.0f}s old (> {epoch_duration_seconds}s epoch), resetting to current time")
            final_genesis['config']['epoch_timestamp_ms'] = current_timestamp_ms
        else:
            print(f"  📅 Keeping existing epoch_timestamp_ms: {existing_timestamp} (elapsed: {elapsed_seconds:.0f}s < {epoch_duration_seconds}s)")
    
    # ĐẢM BẢO các trường khác được giữ nguyên
    # alloc, config, etc. đã được copy từ genesis gốc

    # Save updated genesis.json - BACKUP TRƯỚC
    print(f"💾 Saving updated genesis.json to: {genesis_path} (BACKUP FIRST)")
    print(f"   📊 Validators: {len(new_validators)} (updated)")
    print(f"   📊 Alloc: {len(final_genesis.get('alloc', []))} (preserved)")
    print(f"   🔒 Backup file gốc trước khi ghi")

    # Ghi với backup an toàn
    save_genesis_backup_first(genesis_path, final_genesis)

    print(f"✅ Successfully synced keys for {len(new_validators)} validators")
    print(f"   💡 Go Master sẽ init genesis với validators này khi khởi động")
    print(f"   🔒 Toàn bộ alloc và cấu trúc khác được bảo toàn 100%!")

    # CẬP NHẬT committee.json với stake và threshold chính xác
    committee_path = "config/committee.json"
    try:
        with open(committee_path, 'r') as f:
            committee = json.load(f)

        # Update committee với total_stake và threshold từ final_genesis
        committee['total_stake'] = final_genesis.get('total_stake', 4)
        committee['quorum_threshold'] = final_genesis.get('quorum_threshold', 3)
        committee['validity_threshold'] = final_genesis.get('validity_threshold', 2)
        
        # CRITICAL: Sync epoch_timestamp_ms to committee to ensure Genesis Hash matches
        if 'config' in final_genesis and 'epoch_timestamp_ms' in final_genesis['config']:
            committee['epoch_timestamp_ms'] = final_genesis['config']['epoch_timestamp_ms']
            print(f"   📅 Synced epoch_timestamp_ms to committee: {committee['epoch_timestamp_ms']}")

        # Update stake cho từng authority
        for idx, authority in enumerate(committee.get('authorities', [])):
            if idx < len(new_validators):
                validator = new_validators[idx]
                total_staked_str = validator.get('total_staked_amount', '0')
                try:
                    stake_wei = int(total_staked_str)
                    stake_tokens = stake_wei // (10**18)
                    authority['stake'] = stake_tokens
                except ValueError:
                    print(f"  ⚠️  Invalid stake for authority {idx}")

        with open(committee_path, 'w') as f:
            json.dump(committee, f, indent=2)

        print(f"✅ Updated committee.json with correct stake, thresholds, and timestamp")
        print(f"   📊 Committee total_stake: {committee.get('total_stake')}")
        print(f"   📊 Committee quorum_threshold: {committee.get('quorum_threshold')}")
        print(f"   📊 Committee validity_threshold: {committee.get('validity_threshold')}")

    except Exception as e:
        print(f"⚠️  Could not update committee.json: {e}")

if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("Usage: python3 sync_committee_to_genesis.py <committee.json> <genesis.json>")
        sys.exit(1)
    
    committee_path = sys.argv[1]
    genesis_path = sys.argv[2]
    
    if not os.path.exists(committee_path):
        print(f"❌ Error: Committee file not found: {committee_path}")
        sys.exit(1)
    
    if not os.path.exists(genesis_path):
        print(f"❌ Error: Genesis file not found: {genesis_path}")
        sys.exit(1)
    
    try:
        sync_committee_to_genesis(committee_path, genesis_path)
    except Exception as e:
        print(f"❌ Error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)

