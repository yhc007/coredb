/**
 * Energy Collector for CoreDB
 * 전력 API에서 데이터를 수집하여 CoreDB에 저장
 */

const ENERGY_API_URL = 'http://192.168.10.44/DashboardData/GetAPIinputData';
const COREDB_URL = 'http://localhost:9043';
const COLLECT_INTERVAL_MS = 5 * 60 * 1000; // 5분

async function fetchEnergyData(sdate, edate) {
    const url = `${ENERGY_API_URL}?sdate=${sdate}&edate=${edate}`;
    try {
        const response = await fetch(url, { timeout: 10000 });
        if (!response.ok) {
            console.error(`Energy API error: ${response.status}`);
            return [];
        }
        return await response.json();
    } catch (error) {
        console.error('Energy API fetch error:', error.message);
        return [];
    }
}

async function insertToCoreDB(deviceName, timestamp, usageKwh, powerKw) {
    const id = `${deviceName}_${timestamp}`;
    const query = `INSERT INTO energy.readings (id, device_name, ts, usage_kwh, power_kw) VALUES ('${id}', '${deviceName}', ${timestamp}, ${usageKwh}, ${powerKw})`;
    
    try {
        const response = await fetch(`${COREDB_URL}/query`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ query })
        });
        const result = await response.json();
        return result.status === 'success';
    } catch (error) {
        console.error('CoreDB insert error:', error.message);
        return false;
    }
}

function formatDate(date) {
    const y = date.getFullYear();
    const m = String(date.getMonth() + 1).padStart(2, '0');
    const d = String(date.getDate()).padStart(2, '0');
    const h = String(date.getHours()).padStart(2, '0');
    const min = String(date.getMinutes()).padStart(2, '0');
    const s = String(date.getSeconds()).padStart(2, '0');
    return `${y}-${m}-${d}T${h}:${min}:${s}`;
}

async function collectAndStore() {
    const now = new Date();
    const fiveMinAgo = new Date(now.getTime() - 5 * 60 * 1000);
    
    const sdate = formatDate(fiveMinAgo);
    const edate = formatDate(now);
    
    console.log(`[${new Date().toISOString()}] Collecting energy data: ${sdate} ~ ${edate}`);
    
    const energyData = await fetchEnergyData(sdate, edate);
    
    if (energyData.length === 0) {
        console.log('No energy data received');
        return;
    }
    
    console.log(`Received ${energyData.length} records`);
    
    let inserted = 0;
    for (const record of energyData) {
        const deviceName = record.deviceName || record.DeviceName || 'unknown';
        const timestamp = new Date(record.timestamp || record.Timestamp).getTime();
        const usageKwh = record['사용량'] || record.usage || 0;
        const powerKw = record['유효전력'] || record.power || 0;
        
        if (await insertToCoreDB(deviceName, timestamp, usageKwh, powerKw)) {
            inserted++;
        }
    }
    
    console.log(`Inserted ${inserted}/${energyData.length} records to CoreDB`);
}

async function main() {
    console.log('🔋 Energy Collector started');
    console.log(`API: ${ENERGY_API_URL}`);
    console.log(`CoreDB: ${COREDB_URL}`);
    console.log(`Interval: ${COLLECT_INTERVAL_MS / 1000}s`);
    
    // 시작 시 바로 한 번 수집
    await collectAndStore();
    
    // 주기적으로 수집
    setInterval(collectAndStore, COLLECT_INTERVAL_MS);
}

main().catch(console.error);
