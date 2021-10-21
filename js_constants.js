
"use strict";




// HTTP HEADER FIELDS
exports.CONST_ACCESSCODE_PARAMETER          = 'ac';
exports.CONST_DEVICE_FINGERPRINT            = 'fp';
exports.CONST_SESSION_ID_PARAMETER          = 's';
exports.CONST_PARTY_ID_PARAMETER            = 'p';
exports.CONST_APK_FINGERPRINT_PARAMETER     = 'ak';
exports.CONST_ACTOR_TYPE                    = 'at';   // isGCS
////////////////EO-HTTP HEADER FIELDS


// Reply-Fields with AUTH <--> COMM-Servers
exports.CONST_CS_CMD_INFO                       = 'a';
exports.CONST_CS_CMD_LOGIN_REQUEST              = 'b';
exports.CONST_CS_CMD_LOGOUT_REQUEST             = 'c';

exports.CONST_CS_ACCOUNT_ID                     = 'a';
exports.CONST_CS_GROUP_ID                       = 'b';
exports.CONST_CS_SENDER_ID                      = 'c';
exports.CONST_CS_LOGIN_TEMP_KEY                 = 'f';
exports.CONST_CS_ERROR                          = 'e';   
exports.CONST_CS_SERVER_PUBLIC_HOST             = 'g'; 
exports.CONST_CS_SERVER_PORT                    = 'h';  
exports.CONST_CS_REQUEST_ID                     = 'r';


exports.CONST_WS_SENDER_ID                      = 'sd';
exports.CONST_WS_PARTY_ID                       = 'pd';
exports.CONST_WS_TARGET_ID                      = 'tg';
exports.CONST_WS_PAYLOAD                        = 'ms';
exports.CONST_WS_MESSAGE_ID                     = "mt";
exports.CONST_WS_MSG_ROUTING                    = "ty";
exports.CONST_WS_MSG_ROUTING_GROUP              = "g";
exports.CONST_WS_MSG_ROUTING_INDIVIDUAL         = "i";
exports.CONST_WS_MSG_ROUTING_SYSTEM             = "s";

/////////////EO-Reply with COMM-Servers


// Error numbers
exports.CONST_ERROR_NON                         = 0;   
exports.CONST_ERROR_INVALID_DATA                = 1;   
exports.CONST_ERROR_DATA_NOT_FOUND              = 2;   
exports.CONST_ERROR_DATA_DATABASE_ERROR         = 3;  
exports.CONST_ERROR_SERVER_NOT_AVAILABLE        = 4;  
exports.CONST_ERROR_DATA_UNKNOWN_ERROR          = 999;   


exports.CONST_TYPE_AndruavSystem_LoadTasks		        = 9001;
exports.CONST_TYPE_AndruavSystem_SaveTasks		        = 9002;
exports.CONST_TYPE_AndruavSystem_DeleteTasks	        = 9003;
exports.CONST_TYPE_AndruavSystem_DisableTasks	        = 9004;
exports.CONST_TYPE_AndruavSystem_Ping                   = 9005;
exports.CONST_TYPE_AndruavSystem_LogoutCommServer       = 9006;
exports.CONST_TYPE_AndruavSystem_ConnectedCommServer    = 9007;

// Validation
exports.CONST_ACCESSCODE_MAX_LENGTH = 200;
exports.CONST_FINGERPRINT_MAX_LENGTH = 200;
exports.CONST_SESSION_MAX_LENGTH = 200;
exports.CONST_SENDERID_MAX_LENGTH = 200;

