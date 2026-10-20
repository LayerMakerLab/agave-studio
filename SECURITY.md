# Security

Agave Studio runs on your own computer. The gateway binds to 127.0.0.1:4170 (test mode can use another local port),
has no sign-in, and checks the Host, Origin and content type of every request so other websites cannot drive it.
Every change through the API (a paid run, a key) also needs a cookie made fresh at each start and given only to a
browser loading the studio's pages, so another program cannot start a paid job by calling the API directly.

## Reporting a problem

Please report vulnerabilities privately through GitHub: open the repository's **Security** tab and choose
**Report a vulnerability**. Do not open a public issue for a security problem.

Useful details: what you found, the steps to reproduce it, and what an attacker could do with it.

## In scope

- Anything that lets another website, app or network host reach the gateway or read your keys.
- Path traversal, command injection or server-side request forgery in the gateway.
- Script injection in the studio.

Keys you paste are stored in `config/keys.env` on your computer (owner-only, mode 0600), and the `data/` folder with
your outputs and job history is owner-only (0700). Any program running as your user can read those files. While the
gateway runs, any program on this computer, under any account, can read from it on 127.0.0.1 (there is no sign-in).
The launch cookie stops programs that simply call the API; it is not a sign-in, and a program set on imitating a
browser could still get one. Use Agave on a computer whose accounts you trust.

Use a current ffmpeg (6.0 or newer). The editor passes your own clips and provider files to ffmpeg, and older
releases had demuxer bugs with crafted media.
