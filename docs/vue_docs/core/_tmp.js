const fs = require('fs');
const path = 'docs/vue_docs/core/03_会话状态与消息流.md';
const content = fs.readFileSync(path, 'utf8');
console.log('Current length:', content.length);
