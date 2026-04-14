const Database = require('better-sqlite3');
const dbPath = 'G:\\VCPChat\\VCPDistributedServer\\Plugin\\VCPMobileSync\\sync_state.db';
const db = new Database(dbPath);

const results = db.prepare('SELECT * FROM entity_index').all();
console.log('Total entity_index count:', results.length);
results.filter(r => r.id.includes('topic_1775928718930')).forEach(r => console.log(JSON.stringify(r)));

const msgResults = db.prepare('SELECT count(*) as count FROM message_index').get();
console.log('Total message_index count:', msgResults.count);

const oneTopicMsgs = db.prepare('SELECT * FROM message_index LIMIT 5').all();
console.log('Example message_index rows:', JSON.stringify(oneTopicMsgs));

const targetTopicMsgs = db.prepare('SELECT count(*) as count FROM message_index WHERE topic_id = ?').get('topic_1775928718930');
console.log('Messages count for topic_1775928718930:', targetTopicMsgs.count);
