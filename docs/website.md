# Website and search indexing

The public site is https://tssc67.github.io/mobi-reader/.
Source files live in `site/`. The Pages workflow adds the shared `assets/` directory
(including fonts and license notices) and deploys the static artifact on changes
to the site, assets, or workflow. No JavaScript framework or build dependencies
are required. Run the workflow manually when needed.

## Google Search Console

Use a **URL-prefix** property with the exact value:

```text
https://tssc67.github.io/mobi-reader/
```

Select HTML file verification. Google gives you an account-specific file such as
`google0123456789abcdef.html`. Keep its exact filename and contents and place it
directly in `site/`. Commit and push it; after Pages deploys, its URL will be:

```text
https://tssc67.github.io/mobi-reader/google0123456789abcdef.html
```

That filename is only an example; do not create it or invent a verification
token. Open the actual URL to check it, then click Verify in Search Console.
Keep the file deployed after verification. Alternatively, Google's HTML meta-tag
method can be added to `site/index.html` using the exact supplied token.

Submit `https://tssc67.github.io/mobi-reader/sitemap.xml` as the sitemap and request
indexing of the homepage through URL Inspection. Verification does not guarantee
indexing or ranking. Canonical URL, description, social metadata, and application
structured data are included. A project-path robots.txt does not control the host:
crawlers only use robots.txt at the origin root.

For a future custom domain, configure GitHub Pages and DNS first, then update the
canonical URL, social URL, structured data, and sitemap. A Search Console Domain
property is verified through DNS; the GitHub URL uses a URL-prefix property.
