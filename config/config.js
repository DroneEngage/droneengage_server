var moment = require('moment');
var year = moment().format('YYYY');

module.exports = {
    title: 'mysite',
    version: '1.0.0',
    year: year,
    copyright: '&copy; mysite ' + year + '. All rights reserved.',
    timeFormat: 'MM/DD/YYYY h:mm:ss A',
    mysqlTimeFormat: 'YYYY-MM-DD HH:mm:ss',
    publicFolder: '/public',
    session: {
        name: 'mysite',
        secret: 'abcdefghijklmnop'
    }
};