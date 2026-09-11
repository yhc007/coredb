use bloomfilter::Bloom;
use crate::schema::{PartitionKey, CassandraValue};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use serde::{Serialize, Deserialize, Serializer, Deserializer};

/// 블룸 필터 래퍼
#[derive(Debug, Clone)]
pub struct BloomFilter {
    bloom: Bloom<Vec<u8>>,
    // 직렬화를 위한 설정 저장
    expected_items: usize,
    false_positive_rate: f64,
}

// PartialEq implementation for SSTable compatibility
impl PartialEq for BloomFilter {
    fn eq(&self, other: &Self) -> bool {
        self.expected_items == other.expected_items &&
        self.false_positive_rate == other.false_positive_rate
    }
}

/// `bloomfilter` 크레이트가 받아들이는 오탐률 범위. 밖이면 패닉한다.
const DEFAULT_FP_RATE: f64 = 0.01;

impl BloomFilter {
    /// 인자를 크레이트가 받아들이는 범위로 조인 뒤 만든다.
    ///
    /// 쓰기 경로는 `items_count = 0`을 막고 있었지만, 디스크에서 읽어 올리는
    /// 경로(`Deserialize`)에는 가드가 없었다. SSTable에 적힌 오탐률이 0이면
    /// `Bloom::new_for_fp_rate`가 `fp_p > 0.0 && fp_p < 1.0`에서 패닉하고,
    /// 그 호출이 기동 경로에 있어 **데이터베이스가 아예 뜨지 못한다**.
    /// 실제로 운영 인스턴스가 이 패닉으로 크래시 루프에 빠졌다.
    ///
    /// 블룸 필터는 디스크 읽기를 줄이려는 장치일 뿐이라, 인자가 어긋나면
    /// 최악이라도 읽기가 늘 뿐 답이 틀리지는 않는다. 기동을 막는 것보다 낫다.
    pub fn new(expected_items: u64, false_positive_rate: f64) -> Self {
        let fp_rate = if false_positive_rate.is_finite()
            && false_positive_rate > 0.0
            && false_positive_rate < 1.0
        {
            false_positive_rate
        } else {
            DEFAULT_FP_RATE
        };
        let items = (expected_items as usize).max(1);
        Self {
            bloom: Bloom::new_for_fp_rate(items, fp_rate)
                .expect("bloom filter args are clamped into the accepted range above"),
            expected_items: items,
            false_positive_rate: fp_rate,
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

// Custom Serialize implementation
impl Serialize for BloomFilter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("BloomFilter", 2)?;
        state.serialize_field("expected_items", &self.expected_items)?;
        state.serialize_field("false_positive_rate", &self.false_positive_rate)?;
        state.end()
    }
}

// Custom Deserialize implementation
impl<'de> Deserialize<'de> for BloomFilter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct BloomFilterData {
            expected_items: usize,
            false_positive_rate: f64,
        }
        
        let data = BloomFilterData::deserialize(deserializer)?;
        Ok(BloomFilter::new(data.expected_items as u64, data.false_positive_rate))
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
            // HashMap을 정렬하여 해시
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            for k in keys {
                k.hash(state);
                hash_cassandra_value(m.get(k).unwrap(), state);
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
        CassandraValue::Counter(c) => {
            state.write_u8(12);
            c.hash(state);
        },
        CassandraValue::UDT(fields) => {
            state.write_u8(13);
            // UDT 필드들을 정렬하여 해시
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            for k in keys {
                k.hash(state);
                hash_cassandra_value(fields.get(k).unwrap(), state);
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
