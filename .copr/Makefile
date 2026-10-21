# COPR's "make srpm" build method (Fedora COPR, SCM source): downloads the
# spec's sources, which are the release's tarballs, so build only once
# the release in packaging/fedora/nebula-player.spec is published.
spec := packaging/fedora/nebula-player.spec

srpm:
	dnf -y install rpmdevtools rpm-build
	spectool -g -C $(outdir) $(spec)
	rpmbuild -bs --define "_sourcedir $(outdir)" --define "_srcrpmdir $(outdir)" $(spec)
