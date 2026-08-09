const https = require('https');
const WebSocket = require('ws');

const partyID = '122111';

const options = {
    hostname: '127.0.0.1',
    port: 19408,
    path: '/agent/al',
    method: 'POST',
    headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
    },
    rejectUnauthorized: false,
};

// Exact same params as Android app LoginClient.ValidateAccount
const postData = "acc=single@airgap.droneengage.com&pwd=test&gr=1&app=andruav&ex=Andruav+Mobile";

console.log("Sending Android-style login request (form-urlencoded)...");
console.log("POST data:", postData);

const req = https.request(options, (res) => {
    let data = '';
    res.on('data', (chunk) => { data += chunk; });
    res.on('end', () => {
        console.log("Auth server response:", data);
        try {
            const parsedData = JSON.parse(data);
            if (parsedData.e !== 0) {
                console.error('Login failed:', parsedData);
                return;
            }
            console.log("Login success!");
            console.log("  cs:", JSON.stringify(parsedData.cs));
            
            if (parsedData.cs) {
                const server_ip = parsedData.cs.g;
                const server_port = parsedData.cs.h;
                const server_AuthKey = parsedData.cs.f;
                    
                // Android app builds URL as: wss://IP:PORT?f=KEY&s=PARTY_ID
                const wsUrl = `wss://${server_ip}:${server_port}?f=${server_AuthKey}&s=${partyID}`;
                console.log("\nConnecting to WebSocket (Android-style URL):", wsUrl);
                
                const ws = new WebSocket(wsUrl, { rejectUnauthorized: false });
                
                ws.on('open', () => {
                    console.log("WebSocket OPENED!");
                });
                
                ws.on('message', (msg) => {
                    console.log("WebSocket message:", msg.toString());
                });
                
                ws.on('close', (code, reason) => {
                    console.log(`WebSocket closed: code=${code}, reason=${reason}`);
                });
                
                ws.on('error', (err) => {
                    console.error('WebSocket error:', err.message);
                });
                
                setTimeout(() => {
                    ws.close();
                    process.exit(0);
                }, 5000);
            }
        } catch (e) {
            console.error("Parse error:", e);
        }
    });
});

req.on('error', (err) => {
    console.error('Request error:', err);
});

req.write(postData);
req.end();
