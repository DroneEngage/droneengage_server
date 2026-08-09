// https://github.com/sindresorhus/strip-json-comments

'use strict';

const c_singleComment = 1;
const c_multiComment = 2;
const c_stripWithoutWhitespace = () => '';
const c_stripWithWhitespace = (str, start, end) => str.slice(start, end).replace(/\S/g, ' ');

module.exports = (str, p_opts) => {
	p_opts = p_opts || {};

	const c_strip = p_opts.whitespace === false ? c_stripWithoutWhitespace : c_stripWithWhitespace;

	let v_insideString = false;
	let v_insideComment = false;
	let v_offset = 0;
	let v_ret = '';

	for (let i = 0; i < str.length; i++) {
		const c_currentChar = str[i];
		const c_nextChar = str[i + 1];

		if (!v_insideComment && c_currentChar === '"') {
			const escaped = str[i - 1] === '\\' && str[i - 2] !== '\\';
			if (!escaped) {
				v_insideString = !v_insideString;
			}
		}

		if (v_insideString) {
			continue;
		}

		if (!v_insideComment && c_currentChar + c_nextChar === '//') {
			v_ret += str.slice(v_offset, i);
			v_offset = i;
			v_insideComment = c_singleComment;
			i++;
		} else if (v_insideComment === c_singleComment && c_currentChar + c_nextChar === '\r\n') {
			i++;
			v_insideComment = false;
			v_ret += c_strip(str, v_offset, i);
			v_offset = i;
			continue;
		} else if (v_insideComment === c_singleComment && c_currentChar === '\n') {
			v_insideComment = false;
			v_ret += c_strip(str, v_offset, i);
			v_offset = i;
		} else if (!v_insideComment && c_currentChar + c_nextChar === '/*') {
			v_ret += str.slice(v_offset, i);
			v_offset = i;
			v_insideComment = c_multiComment;
			i++;
			continue;
		} else if (v_insideComment === c_multiComment && c_currentChar + c_nextChar === '*/') {
			i++;
			v_insideComment = false;
			v_ret += c_strip(str, v_offset, i + 1);
			v_offset = i + 1;
			continue;
		}
	}

	return v_ret + (v_insideComment ? c_strip(str.substr(v_offset)) : str.substr(v_offset));
};