"use strict";

// AndruaveMessageID
const AndruavMessageTypes = {

 CONST_TYPE_AndruavMessage_ID : 1004,
 CONST_TYPE_AndruavMessage_RemoteExecute : 1005,
 CONST_TYPE_AndruavMessage_Telemetry : 1007,
 CONST_TYPE_AndruavMessage_Error : 1008,
 CONST_TYPE_AndruavMessage_FlightControl : 1010,
 CONST_TYPE_AndruavMessage_VideoFrame : 1014,
 CONST_TYPE_AndruavMessage_IMG : 1006,
 CONST_TYPE_AndruavMessage_GPS : 1002,
 CONST_TYPE_AndruavMessage_POW : 1003,
 CONST_TYPE_AndruavMessage_CameraList : 1012, // RX: {"tg":"GCS1","sd":"zxcv","ty":"c","gr":"1","cm":"i","mt":1012,"ms":"{\"E\":2,\"P\":0,\"I\":\"zxcv\"}"}
 CONST_TYPE_AndruavMessage_IMU : 1013,
 CONST_TYPE_AndruavMessage_BinaryIMU : 1013,
 CONST_TYPE_AndruavMessage_IMUStatistics : 1016,
 CONST_TYPE_AndruavMessage_DroneReport : 1020,
 CONST_TYPE_AndruavMessage_HomeLocation : 1022,
 CONST_TYPE_AndruavMessage_GeoFence : 1023,
 CONST_TYPE_AndruavMessage_ExternalGeoFence : 1024,
 CONST_TYPE_AndruavMessage_GEOFenceHit : 1025,
 CONST_TYPE_AndruavMessage_WayPoints : 1027,
 CONST_TYPE_AndruavMessage_ExternalCommand_WayPoints : 1028,
 CONST_TYPE_AndruavMessage_GeoFenceAttachStatus : 1029,
 CONST_TYPE_AndruavMessage_Arm : 1030,
 CONST_TYPE_AndruavMessage_ChangeAltitude : 1031,
 CONST_TYPE_AndruavMessage_Land : 1032,
 CONST_TYPE_AndruavMessage_DoYAW : 1035,
 CONST_TYPE_AndruavMessage_Signaling : 1021,
 CONST_TYPE_AndruavMessage_GuidedPoint : 1033,
 CONST_TYPE_AndruavMessage_CirclePoint : 1034,
 CONST_TYPE_AndruavMessage_NAV_INFO : 1036,
 CONST_TYPE_AndruavMessage_DistinationLocation : 1037,
 CONST_TYPE_AndruavMessage_ChangeSpeed : 1040,
 CONST_TYPE_AndruavMessage_Ctrl_Camera : 1041,
// CODEBLOCK_START
 CONST_TYPE_AndruavMessage_TrackingTarget : 1042,
 CONST_TYPE_AndruavMessage_TrackingTargetLocation : 1043,
 CONST_TYPE_AndruavMessage_TargetLost : 1044,
// CODEBLOCK_END
 CONST_TYPE_AndruavMessage_GimbalCtrl : 1045,
 CONST_TYPE_AndruavMessage_UploadWayPoints : 1046,
 CONST_TYPE_AndruavMessage_RemoteControlSettings : 1047,
 CONST_TYPE_AndruavMessage_SetHomeLocation : 1048,
 CONST_TYPE_AndruavMessage_CameraZoom : 1049,
 CONST_TYPE_AndruavMessage_CameraSwitch : 1050,
 CONST_TYPE_AndruavMessage_CameraFlash : 1051,
 CONST_TYPE_AndruavMessage_RemoteControl2 : 1052,
 CONST_TYPE_AndruavMessage_SensorsStatus : 1053,
// CODEBLOCK_START
 CONST_TYPE_AndruavMessage_FollowHim_Request : 1054,
 CONST_TYPE_AndruavMessage_FollowMe_Guided : 1055,
 CONST_TYPE_AndruavMessage_MakeSwarm : 1056,
 CONST_TYPE_AndruavMessage_SwarmReport : 1057,
 CONST_TYPE_AndruavMessage_UpdateSwarm : 1058,
// CODEBLOCK_END

 CONST_TYPE_AndruavMessage_CommSignalsStatus   : 1059,
 CONST_TYPE_AndruavMessage_Sync_EventFire      : 1061,
 CONST_TYPE_AndruavMessage_SearchTargetList    : 1062,
 CONST_TYPE_AndruavMessage_UdpProxy_Info       : 1071,
 CONST_TYPE_AndruavMessage_Unit_Name           : 1072,


// Binary Messages
 CONST_TYPE_AndruavMessage_LightTelemetry      : 2022,

// new Andruav Messages 2019
 CONST_TYPE_AndruavMessage_ServoChannel        : 6001,
 CONST_TYPE_AndruavBinaryMessage_ServoOutput   : 6501,
 CONST_TYPE_AndruavBinaryMessage_Mavlink       : 6502,

// System Messages
 CONST_TYPE_AndruavSystem_LoadTasks            : 9001,
 CONST_TYPE_AndruavSystem_SaveTasks            : 9002,
 CONST_TYPE_AndruavSystem_DeleteTasks          : 9003,
 CONST_TYPE_AndruavSystem_DisableTasks         : 9004,
 CONST_TYPE_AndruavSystem_LogoutCommServer     : 9006,
 CONST_TYPE_AndruavSystem_ConnectedCommServer  : 9007,
};




module.exports = AndruavMessageTypes;
