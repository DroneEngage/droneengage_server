/*************************************************************************************
 * 
 *   A N D R U A V - T A S K S   [Offline Tasks]     JAVASCRIPT  LIB
 * 
 *   Author: Mohammad S. Hefny
 * 
 *   Date:   17 Jun 2016
 * 
 *   Update: 27 Jun 2016  // adding Access Code
 *   Update: 01 Jul 2016  // fixes
 *   Update: 09 SEP 2016  // fixes in Hajj Live
 * 
 * 
 */

const mysql = require('mysql2');
var v_dbPool;


exports.fn_initTasks = function () {
	if (global.m_serverconfig.m_configuration.ignoreLoadingTasks === false) {
		// read config server info in c_serverconfig.JSONconfig


		// Database Connection
		v_dbPool = mysql.createPool({
			connectionLimit: 18, //important
			queueLimit: 19,
			host: global.m_serverconfig.m_configuration.dbIP,
			user: global.m_serverconfig.m_configuration.dbuser,
			password: global.m_serverconfig.m_configuration.dbpassword,
			database: global.m_serverconfig.m_configuration.dbdatabase,
			debug: false
		});
	}
}

String.prototype.fn_protectedFromInjection = function () {
	var target = this;
	return target.replace(new RegExp("--", 'g'), " ");
};

function _getTasksSQLWhere(largerThan_SID, accountID, party_sid, groupName, sender, receiver, messageType, task, isPermanent, enabled) {
	return fn_getTasksSQLWhere2(largerThan_SID, accountID, party_sid, groupName, sender, receiver, messageType, task, isPermanent, 1)
}

function fn_getTasksSQLWhere2(largerThan_SID, accountID, party_sid, groupName, sender, receiver, messageType, task, isPermanent, enabled) {
	var sql = "";
	var putAnd = false;
	if ((largerThan_SID != null) && (largerThan_SID != undefined) && (typeof (largerThan_SID) == 'string')) {
		sql = sql + " SID  > " + largerThan_SID.fn_protectedFromInjection();
		putAnd = true;
	}

	if ((accountID != null) && (accountID != undefined) && (typeof (largerThan_SID) == 'string')) {
		if (putAnd) sql = sql + " AND ";
		sql = sql + " accountID = '" + accountID.fn_protectedFromInjection() + "'";
		putAnd = true;
	}

	if ((party_sid != null) && (party_sid != undefined)) {
		if (putAnd) sql = sql + " AND ";
		sql = sql + " party_sid = '" + party_sid.fn_protectedFromInjection() + "'";
		putAnd = true;
	}

	if ((groupName != null) && (groupName != undefined)) {
		if (putAnd) sql = sql + " AND ";
		sql = sql + " groupName = '" + groupName.fn_protectedFromInjection() + "'";
		putAnd = true;
	}

	if ((sender != null) && (sender != undefined)) {
		if (putAnd) sql = sql + " AND ";
		sql = sql + " sender = '" + sender.fn_protectedFromInjection() + "'";
		putAnd = true;
	}

	if ((receiver != null) && (receiver != undefined)) {
		if (putAnd) sql = sql + " AND ";
		sql = sql + " receiver = '" + receiver.fn_protectedFromInjection() + "'";
		putAnd = true;
	}

	if ((messageType != null) && (messageType != undefined)) {
		if (putAnd) sql = sql + " AND ";
		sql = sql + " messageType = '" + messageType.toString().fn_protectedFromInjection() + "'";
		putAnd = true;
	}

	if ((task != null) && (task != undefined)) {
		if (putAnd) sql = sql + " AND ";
		sql = sql + " task = '" + task.fn_protectedFromInjection() + "'";
		putAnd = true;
	}

	if ((isPermanent != null) && (isPermanent != undefined)) {
		if (putAnd) sql = sql + " AND ";
		sql = sql + " isPermanent = " + isPermanent;
		putAnd = true;
	}

	if ((enabled != null) && (enabled != undefined)) {
		if (putAnd) sql = sql + " AND ";
		sql = sql + " enabled = " + enabled.toString();
		putAnd = true;
	}
	return sql;
}

/*
 * 
 * largerThan_SID: SID of the last recieved data use 0 to get everything 
 * 
 * 
 * returns ASYC: {count:rows.length}
 */
exports.fn_fn_get_tasks_count = function fn_fn_get_tasks_count(params) {

	try {

		v_dbPool.getConnection(function (err, dbConnection) {
			if (err) {
				if (dbConnection != null) dbConnection.release();
				//console.log(err);
				//console.log("err:" + err);

				if (params.errfunc != null) params.errfunc(err);

				return;
			}

			var sql = "SELECT COUNT('SID') AS COUNT FROM `task` WHERE ";

			sql = sql + _getTasksSQLWhere(params.largerThan_SID, params.accountID, params.party_sid, params.groupName, params.sender, params.receiver, params.messageType, params.task, params.isPermanent);



			//console.log ("SQL:" + sql);
			dbConnection.query(sql, function (err, rows) {
				//release connection now
				dbConnection.release();

				if (err) {
					console.log("Database error");
					console.log(err);

					if (params.errfunc != null) params.errfunc(err);
					return;
				}

				if (rows.length == 0) params.resultfunc(0);


				params.resultfunc(rows[0].COUNT);

			});
		});
	}
	catch (e) {
		console.log(e);
	}


}





/*
 * 
 * largerThan_SID: SID of the last recieved data use 0 to get everything 
 * 
 * 
 * returns ASYC: {rows} of SIDs & MessageType ONLY.
 */
exports.fn_fn_get_tasks_sids = function fn_fn_get_tasks_sids(params) {
	//{resultfunc,errfunc,largerThan_SID, party_sid,sender,receiver,messageType,task,isPermanent}

	try {

		v_dbPool.getConnection(function (err, dbConnection) {
			if (err) {
				// TEST //if (dbConnection) dbConnection.release();
				//console.log(err);
				//console.log("err:" + err);

				if (params.errfunc != null) params.errfunc(err);

				return;
			}

			var sql = "SELECT SID , `messageType` FROM `task` WHERE ";


			sql = sql + _getTasksSQLWhere(params.largerThan_SID, params.accountID, params.party_sid, params.groupName, params.sender, params.receiver, params.messageType, params.task, params.isPermanent);

			console.log("SQL:" + sql);
			dbConnection.query(sql, function (err, rows) {
				//release connection now
				if (dbConnection != null) dbConnection.release();

				if (err) {
					console.log("Database error");
					console.log(err);

					if (params.errfunc != undefined) params.errfunc(err);
					return;
				}


				/*
				 * 	Reply example:
					RowDataPacket { SID: 11, messageType: 1024 }
					RowDataPacket { SID: 12, messageType: 1024 }
					RowDataPacket { SID: 13, messageType: 1024 }
					RowDataPacket { SID: 14, messageType: 1026 }
					RowDataPacket { SID: 22, messageType: 1024 }
				*/
				if (params.resultfunc != undefined) params.resultfunc(rows);


			});
		});
	}
	catch (e) {
		console.log(e);
	}

}



/*
 * 
 * largerThan_SID: SID of the last recieved data use 0 to get everything 
 * 
 * 
 * returns ASYC: rows
 */
exports.fn_get_tasks = function fn_get_tasks(params) {
	//{resultfunc,errfunc,largerThan_SID, party_sid,sender,receiver,messageType,task,isPermanent}

	try {

		if ((params.accountID == null) || (params.accountID.length == 0) || (params.accountID.CONST_ACCESSCODE_MAX_LENGTH >= 0) || (params.accountID.fn_isEmail() === false)) {
			err = 'missing accountID';
			return;
		}

		v_dbPool.getConnection(function (err, dbConnection) {



			if (err) {
				// TEST //if (dbConnection) dbConnection.release();
				console.log(err);
				console.log("err:" + err);
				if (dbConnection != null) dbConnection.release();
				if (params.errfunc != null) params.errfunc(err);

				return;
			}

			var sql = "SELECT `SID`, `party_sid`, `groupName`, `sender`, `receiver`, `messageType`, `task`, `isPermanent`, `Lastprocessed_Time`, `Creation_Time` FROM `task` WHERE ";

			sql = sql + _getTasksSQLWhere(params.largerThan_SID, params.accountID, params.party_sid, params.groupName, params.sender, params.receiver, params.messageType, params.task, params.isPermanent);


			console.log("SQL:" + sql);
			dbConnection.query(sql, function (err, rows) {
				//release connection now
				if (dbConnection != null) dbConnection.release();

				if (err) {
					console.log("Database error");
					console.log(err);

					if (params.errfunc != undefined) params.errfunc(err);

					return;
				}

				if (params.resultfunc != undefined) params.resultfunc(rows);


			});
		});
	}
	catch (e) {
		console.log(e);
	}

}



exports.fn_del_tasks = function fn_del_tasks(params) {
	//{resultfunc,errfunc,largerThan_SID, party_sid,sender,receiver,messageType,task,isPermanent}

	try {

		if ((params.accountID == null) || (params.accountID.length == 0) || (params.accountID.CONST_ACCESSCODE_MAX_LENGTH >= 0) || (params.accountID.fn_isEmail() === false)) {
			err = 'missing accountID';
			return;
		}

		v_dbPool.getConnection(function (err, dbConnection) {

			if (err) {
				if (dbConnection != null) dbConnection.release();
				console.log(err);
				console.log("err:" + err);

				if (params.errfunc != null) params.errfunc(err);

				return;
			}

			var sql = "DELETE FROM `task` WHERE";

			sql = sql + fn_getTasksSQLWhere2(params.largerThan_SID, params.accountID, params.party_sid, params.groupName, params.sender, params.receiver, params.messageType, params.task, params.isPermanent, params.enabled);

			console.log("SQL:" + sql);

			dbConnection.query(sql, function (err, rows) {
				//release connection now
				if (dbConnection != null) dbConnection.release();

				if (err) {
					console.log("Database error");
					console.log(err);

					if (params.errfunc != undefined) params.errfunc(err);

					return;
				}

				if (params.resultfunc != undefined) params.resultfunc(rows);


			});
		});
	}
	catch (e) {
		console.log(e);
	}

}



exports.fn_disable_tasks = function fn_disable_tasks(params) {
	//{resultfunc,errfunc,largerThan_SID, party_sid,sender,receiver,messageType,task,isPermanent}

	if ((params.accountID == null) || (params.accountID.length == 0) || (params.accountID.CONST_ACCESSCODE_MAX_LENGTH >= 0) || (params.accountID.fn_isEmail() === false)) {
		err = 'missing accountID';
		return;
	}
	try {

		v_dbPool.getConnection(function (err, dbConnection) {


			if (err) {
				if (dbConnection != null) dbConnection.release();
				console.log("err:" + err);

				if (params.errfunc != null) params.errfunc(err);

				return;
			}

			var sql = "UPDATE `task` SET `enabled` = 0 WHERE";

			sql = sql + fn_getTasksSQLWhere2(params.largerThan_SID, params.accountID, params.party_sid, params.groupName, params.sender, params.receiver, params.messageType, params.task, params.isPermanent, params.enabled);

			console.log("SQL:" + sql);
			dbConnection.query(sql, function (err, res) {
				//release connection now

				if (dbConnection != null) dbConnection.release();

				if (err) {
					console.log("Database error");
					console.log(err);

					if (params.errfunc != undefined) params.errfunc(err);

					return;
				}
				console.log(res);
				if (params.resultfunc != undefined) params.resultfunc(res);


			});
		});
	}
	catch (e) {
		console.log(e);
	}

}


/*
 * 
 * largerThan_SID: SID of the last recieved data use 0 to get everything 
 * 
 * 
 * returns ASYC: rows
 */
exports.fn_add_task = function fn_add_task(params) {

	try {

		//resultfunc,errfunc, party_sid,sender,receiver,messageType,task,isPermanent

		v_dbPool.getConnection(function (err, dbConnection) {
			if (err) {
				// TEST //if (dbConnection) dbConnection.release();
				if (dbConnection != null) dbConnection.release();
				//console.log(JSON.stringify(params));
				console.log("err:" + err);

				if (params.errfunc != null) params.errfunc(err);

				return;
			}

			var accountID_n = "";
			var accountID_v = "";
			var sql_part_sid_n = "";
			var sql_part_sid_v = "";
			var groupName_n = "";
			var groupName_v = "";
			var sender_n = "";
			var sender_v = "";
			var receiver_n = "";
			var receiver_v = "";

			if (params.hasOwnProperty("accountID")) {
				accountID_n = "`accountID`, ";
				accountID_v = "'" + (params.accountID.fn_protectedFromInjection()) + "',";
			}

			if (params.hasOwnProperty("party_sid")) {
				sql_part_sid_n = "`party_sid`, ";
				sql_part_sid_v = "'" + params.party_sid.fn_protectedFromInjection() + "',";
			}

			if (params.hasOwnProperty("groupName")) {
				groupName_n = "`groupName`, ";
				groupName_v = params.groupName.fn_protectedFromInjection() + ",";
			}


			if (params.hasOwnProperty("sender")) {
				sender_n = "`sender`, ";
				sender_v = "'" + params.sender.fn_protectedFromInjection() + "',";
			}

			if (params.hasOwnProperty("receiver")) {
				receiver_n = "`receiver`, ";
				receiver_v = "'" + (params.receiver.fn_protectedFromInjection()) + "',";
			}

			params.messageType.toString().fn_protectedFromInjection();
			params.task.fn_protectedFromInjection();
			params.isPermanent.toString().fn_protectedFromInjection();

			var sql = "INSERT INTO `task`(" + accountID_n + sql_part_sid_n + groupName_n + sender_n + receiver_n + " `messageType`, `task`, `isPermanent`) VALUES (" + accountID_v + sql_part_sid_v + groupName_v + sender_v + receiver_v + params.messageType.toString().fn_protectedFromInjection() + ",'" + params.task.fn_protectedFromInjection() + "'," + params.isPermanent.toString().fn_protectedFromInjection() + ");";




			console.log("SQL:" + sql);
			dbConnection.query(sql, function (err, rows) {
				//release connection now
				if (dbConnection != null) dbConnection.release();

				if (err) {
					console.log("Database error");
					console.log(err);

					if (params.errfunc != undefined) params.errfunc(err);

					return;
				}

				if (params.resultfunc != undefined) params.resultfunc(rows);


			});
		});
	}
	catch (e) {
		console.log(e);
	}

}


