const https = require('https');
const WebSocket = require('ws');

const partyID = '122111';
class MyHTTPSClient {
    constructor() {
        this.ws = null; // Store the WebSocket instance
    }

    sendHTTPSRequest(options, postData, successCallback, errorCallback) {
        const req = https.request(options, (res) => {
            let data = '';

            res.on('data', (chunk) => {
                data += chunk;
            });

            res.on('end', () => {
                if (res.statusCode >= 200 && res.statusCode < 300) {
                    try {
                        console.log(`Reply Data: ${data}`);
                        const parsedData = JSON.parse(data);
                        successCallback(parsedData); // Call success callback with parsed data
                    } catch (parseError) {
                        errorCallback(parseError); // Call error callback if JSON parsing fails
                    }
                } else {
                    errorCallback(new Error(`HTTP request failed with status code: ${res.statusCode}`));
                }
            });
        });

        req.on('error', (err) => {
            errorCallback(err); // Call error callback for request errors
        });

        if (postData) {
            req.write(postData);
        }

        req.end();
    }

    connectWebSocket(url, successCallback, errorCallback, messageCallback, closeCallback) {
        const options = {
            rejectUnauthorized: false, // Be very cautious with this in production!
        };

        this.ws = new WebSocket(url, options); // Store the WebSocket instance

        this.ws.on('open', () => {
            console.log('WebSocket connection opened.');
            successCallback(this.ws); // Pass the websocket object to the callback.
            this.startMessaging(); // Start sending messages after connection is open
        });

        this.ws.on('message', (data) => {
            try {
                const parsedData = JSON.parse(data);
                messageCallback(parsedData); // Call the message callback
            } catch (parseError) {
                console.error("websocket message parse error", parseError);
            }
        });

        this.ws.on('close', (code, reason) => {
            console.log(`WebSocket connection closed with code ${code}: ${reason}`);
            closeCallback(code, reason);
        });

        this.ws.on('error', (err) => {
            console.error('WebSocket error:', err);
            errorCallback(err);
        });
    }

    performRequestAndConnect(options, postData) {
        this.sendHTTPSRequest(
            options,
            postData,
            (responseData) => {
                // Success callback
                console.log('HTTPS request successful:', responseData);

                if (responseData.e !== 0) {
                    console.error('Error in response:', responseData);
                    return;
                }
                const server_ip = responseData.cs.g;
                const server_port_ss = responseData.cs.h;
                const server_AuthKey = responseData.cs.f;
                    
                const wsUrl = `wss://${server_ip}:${server_port_ss}?f=${server_AuthKey}&s=${partyID}`;

                this.connectWebSocket(
                    wsUrl,
                    (ws) => {
                        console.log("Websocket connected");
                        //websocket connected
                    },
                    (wsError) => {
                        console.error('WebSocket connection error:', wsError);
                    },
                    (wsMessage) => {
                        console.log("Websocket Message: ", wsMessage);
                    },
                    (closeCode, closeReason) => {
                        console.log("Websocket Closed: ", closeCode, closeReason);
                    }
                );
            },
            (error) => {
                // Error callback
                console.error('HTTPS request error:', error);
            }
        );
    }

    startMessaging() {
        setInterval(() => {
            if (this.ws && this.ws.readyState === WebSocket.OPEN) {
                const message = { message: 'Data from client', timestamp: Date.now() };
                this.ws.send(JSON.stringify(message));
                console.log(`Sent message: ${JSON.stringify(message)}`);
            } else {
                console.log("Websocket is not open, or ws object is null");
            }
        }, 3000); // Send message every 3 seconds
    }
}

// Example usage:
const client = new MyHTTPSClient('122111');

const options = {
    hostname: '127.0.0.1', // or domain if you have one
    port: 19408, // or whatever port you are using for https.
    path: '/agent/al',
    method: 'POST',
    headers: {
        'Content-Type': 'application/json',
    },
    rejectUnauthorized: false, // Insecure!
};

const postData = JSON.stringify({
    acc: "mhefny@andruav.com",
    pwd: "mhefny",
    gr: "1",
    app: 'andruav',
    ver: "5.0.0.1",
    ex: 'Andruav',
    at: "g",
});

client.performRequestAndConnect(options, postData);