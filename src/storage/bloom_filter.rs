use bloomfilter::Bloom;
use crate::schema::{PartitionKey, CassandraValue};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// 블룸 필터 래퍼
#[derive(Debug, Clone)]
pub struct BloomFilter {
    bloom: Bloom<Vec<u8>>,
}

impl BloomFilter {
    pub fn new(expected_items: u64, false_positive_rate: f64) -> Self {
        Self {
            bloom: Bloom::new_for_fp_rate(expected_items as usize, false_positive_rate).expect("Failed to create bloom filter"),
        }
    }
    
    pub fn add(&mut self, key: &PartitionKey) {
        let key_bytes = self.serialize_key(key);
        self.bloom.set(&key_bytes);
    }
    
    pub fn might_contain(&self, key: &PartitionKey) -> bool {
        let key_bytes = self.serialize_key(key);
        self.bloom.check(&key_bytes)
    }
    
    fn serialize_key(&self, key: &PartitionKey) -> Vec<u8> {
        // 간단한 직렬화 (실제로는 더 효율적인 방법 사용 가능)
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish().to_le_bytes().to_vec()
    }
}

impl Hash for PartitionKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for component in &self.components {
            hash_cassandra_value(component, state);
        }
    }
}

fn hash_cassandra_value<H: Hasher>(value: &CassandraValue, state: &mut H) {
    match value {
        CassandraValue::Text(s) => {
            state.write_u8(0);
            s.hash(state);
        },
        CassandraValue::Int(i) => {
            state.write_u8(1);
            i.hash(state);
        },
        CassandraValue::BigInt(i) => {
            state.write_u8(2);
            i.hash(state);
        },
        CassandraValue::UUID(uuid) => {
            state.write_u8(3);
            uuid.hash(state);
        },
        CassandraValue::Timestamp(t) => {
            state.write_u8(4);
            t.hash(state);
        },
        CassandraValue::Boolean(b) => {
            state.write_u8(5);
            b.hash(state);
        },
        CassandraValue::Double(d) => {
            state.write_u8(6);
            d.to_bits().hash(state);
        },
        CassandraValue::Blob(b) => {
            state.write_u8(7);
            b.hash(state);
        },
        CassandraValue::Null => {
            state.write_u8(8);
        },
        CassandraValue::Map(m) => {
            state.write_u8(9);
            for (k, v) in m {
                hash_cassandra_value(k, state);
                hash_cassandra_value(v, state);
            }
        },
        CassandraValue::List(l) => {
            state.write_u8(10);
            for item in l {
                hash_cassandra_value(item, state);
            }
        },
        CassandraValue::Set(s) => {
            state.write_u8(11);
            for item in s {
                hash_cassandra_value(item, state);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::CassandraValue;
    
    #[test]
    fn test_bloom_filter() {
        let mut bloom = BloomFilter::new(100, 0.01);
        
        let key = PartitionKey {
            components: vec![CassandraValue::Int(42)],
        };
        
        bloom.add(&key);
        assert!(bloom.might_contain(&key));
        
        let other_key = PartitionKey {
            components: vec![CassandraValue::Int(43)],
        };
        
        // 다른 키는 거짓 양성이 발생할 수 있지만, 거짓 음성은 발생하지 않아야 함
        assert!(!bloom.might_contain(&other_key));
    }
}
