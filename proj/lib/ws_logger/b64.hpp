#ifndef __B64_HPP__
#define __B64_HPP__

// https://nachtimwald.com/2017/11/18/base64-encode-and-decode-in-c/

const char b64chars[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

size_t b64_encoded_size(size_t inlen)
{
	size_t ret;

	ret = inlen;
	if (inlen % 3 != 0)
		ret += 3 - (inlen % 3);
	ret /= 3;
	ret *= 4;

	return ret;
}

char *b64_encode(const unsigned char *in, size_t len)
{
	char   *out;
	size_t  elen;
	size_t  i;
	size_t  j;
	size_t  v;

	if (in == NULL || len == 0)
		return NULL;

	elen = b64_encoded_size(len);
	out  = (char*)malloc(elen+1);
	out[elen] = '\0';

	for (i=0, j=0; i<len; i+=3, j+=4) {
		v = in[i];
		v = i+1 < len ? v << 8 | in[i+1] : v << 8;
		v = i+2 < len ? v << 8 | in[i+2] : v << 8;

		out[j]   = b64chars[(v >> 18) & 0x3F];
		out[j+1] = b64chars[(v >> 12) & 0x3F];
		if (i+1 < len) {
			out[j+2] = b64chars[(v >> 6) & 0x3F];
		} else {
			out[j+2] = '=';
		}
		if (i+2 < len) {
			out[j+3] = b64chars[v & 0x3F];
		} else {
			out[j+3] = '=';
		}
	}

	return out;
}

bool isValidUTF8(const char* str) {
    while (*str) {
        unsigned char c = *str++;
        // ASCII
        if (c < 0x80) continue;
        // Non-overlong 2-byte
        if (c < 0xC2) return false;
        if (c <= 0xDF && (*str & 0xC0) == 0x80) { str++; continue; }
        // excluding overlongs
        if (c == 0xE0 && (*str & 0xE0) == 0xA0) return false;
        // Non-overlong 3-byte
        if (c < 0xE1) {
            if ((*str & 0xC0) == 0x80) {
                if ((*(str + 1) & 0xC0) == 0x80) { str += 2; continue; }
            }
        }
        // Maximum valid 3-byte
        if (c <= 0xEF && (*str & 0xC0) == 0x80 && (*(str + 1) & 0xC0) == 0x80) { str += 2; continue; }
        // 4-byte
        if (c <= 0xF4) {
            if ((*str & 0xC0) == 0x80 && (*(str + 1) & 0xC0) == 0x80 && (*(str + 2) & 0xC0) == 0x80) { str += 3; continue; }
        }
        return false;
    }
    return true;
}

#endif