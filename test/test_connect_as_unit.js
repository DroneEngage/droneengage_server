const WebSocket = require('ws');

// Parse command-line arguments using process.argv
const args = process.argv.slice(2);

if (args.length !== 5) {
  console.error(
    'Usage: node test_connect_as_unit.js <ip> <port> <resendRate> <hash> <party_ID>'
  );
  process.exit(1);
}

const ip = args[0];
const port = parseInt(args[1]);
const resendRate = parseFloat(args[2]);
const server_AuthKey = args[3];
const partyID = args[4];

// Validation
if (isNaN(port) || port < 1 || port > 65535) {
  console.error('Invalid port number. Must be between 1 and 65535.');
  process.exit(1);
}

if (isNaN(resendRate) || resendRate <= 0) {
  console.error('Invalid resend rate. Must be a positive number.');
  process.exit(1);
}

const serverUrl = `wss://${ip}:${port}?f=${server_AuthKey}&s=${partyID}&at=d`;

const headers = {};

const options = {
  headers: headers,
  rejectUnauthorized: false,
};

const ws = new WebSocket(serverUrl, options);

ws.on('open', () => {
  console.log('Connected to WebSocket server');
  ws.send(JSON.stringify({ type: 'message', data: 'Hello from client!' }));
});

ws.on('message', (message) => {
  try {
    const parsedMessage = JSON.parse(message);
    console.log('Received message:', parsedMessage);
  } catch (error) {
    console.error('Error parsing message:', error);
    console.log('Raw message:', message);
  }
});

ws.on('close', (code, reason) => {
  console.log(`WebSocket connection closed with code ${code}: ${reason}`);
});

ws.on('error', (error) => {
  console.error('WebSocket error:', error);
});

function keepAlive(ws) {
  if (ws.readyState === WebSocket.OPEN) {
    ws.ping();
    const pongTimeout = setTimeout(() => {
      console.log('Websocket pong timeout');
      ws.terminate();
    }, 5000);

    const closeListener = () => {
      clearTimeout(pongTimeout);
      ws.removeListener('close', closeListener);
      ws.removeListener('pong', pongListener);
    };

    const pongListener = () => {
      console.log('WebSocket connection is alive');
      clearTimeout(pongTimeout);
      ws.removeListener('close', closeListener);
      ws.removeListener('pong', pongListener);
    };

    ws.once('close', closeListener);
    ws.once('pong', pongListener);

    console.log('Sending ping...');
  }
}

//setInterval(keepAlive, 7000, ws);

function sendMessage(data) {
  if (ws.readyState === WebSocket.OPEN) {
    console.log('Sending message:', data);
   // ws.send(JSON.stringify(data));
  } else {
    console.log('Websocket is not open');
  }
}

const json_msg = {
  'ty': 'g',
  'sd': partyID
}

setInterval(sendMessage, resendRate * 1000, json_msg);

process.on('SIGINT', () => {
  console.log('Closing WebSocket connection...');
  ws.close();
  process.exit();
});