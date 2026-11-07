package servers

import (
	"archive/zip"
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"github.com/mcm-panel/mcm/internal/storage"
	"io"
	"io/fs"
	"net/http"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// Errors returned by file operations. These map to friendly HTTP errors in the
// API layer; they never leak raw filesystem or Go error strings.
var (
	// ErrInvalidPath is returned when a requested path escapes the server's
	// data directory or is otherwise unsafe.
	ErrInvalidPath = errors.New("invalid path")
	// ErrPathNotFound is returned when a file or directory does not exist.
	ErrPathNotFound = errors.New("path not found")
	// ErrIsDirectory is returned when a file operation expected a file but got
	// a directory (or vice versa).
	ErrIsDirectory = errors.New("path is a directory")
	// ErrInvalidArchive is returned when a zip archive is malformed or unsafe.
	ErrInvalidArchive = errors.New("invalid archive")
	// ErrDownloadFailed is returned when a URL download fails.
	ErrDownloadFailed = errors.New("download failed")
	// ErrReadTooLarge is returned when a file is too large to read into the web editor.
	ErrReadTooLarge = errors.New("file too large to read")
)

// maxDownloadBytes caps how large a file downloaded from a URL may be (512 MB).
const maxDownloadBytes int64 = 512 << 20

// FileEntry describes a single entry in a server's data directory.
type FileEntry struct {
	Name       string    `json:"name"`
	IsDir      bool      `json:"is_directory"`
	SizeBytes  int64     `json:"size_bytes"`
	ModifiedAt time.Time `json:"modified_at"`
	Revision   string    `json:"revision,omitempty"`
}

// FileContent ties the editor's text to the exact bytes read from disk.
// An empty revision represents a file that does not exist, not an empty file.
type FileContent struct {
	Path     string `json:"path"`
	Content  string `json:"content"`
	Revision string `json:"revision"`
	Exists   bool   `json:"exists"`
}

type FileConflictError struct {
	Current FileContent
}

func (e *FileConflictError) Error() string { return "file changed since it was opened" }

func fileRevision(content string) string {
	return fmt.Sprintf("%x", sha256.Sum256([]byte(content)))
}

func (s *Store) readFileSnapshot(id, rel string) (FileContent, error) {
	content, err := s.ReadFileContent(id, rel)
	if err != nil {
		return FileContent{}, err
	}
	return FileContent{Path: rel, Content: content, Revision: fileRevision(content), Exists: true}, nil
}

func (s *Store) ReadFileSnapshot(id, rel string) (FileContent, error) {
	unlock := s.lockWorld(id, false)
	defer unlock()
	s.fileEditMu.Lock()
	defer s.fileEditMu.Unlock()
	root, err := s.openDataRoot(id)
	if err != nil {
		return FileContent{}, err
	}
	defer root.Close()
	rel, err = safeRel(rel)
	if err != nil {
		return FileContent{}, err
	}
	return s.readFileSnapshotLocked(root, rel)
}

// FileListResult is a directory listing for a server data directory.
type FileListResult struct {
	Path    string      `json:"path"`
	Entries []FileEntry `json:"entries"`
}

// resolvePath is only a lexical helper. Filesystem operations below use rooted handles.
func (s *Store) resolvePath(id, rel string) (string, error) {
	if !storage.ValidID(id) {
		return "", ErrInvalidPath
	}
	clean, err := storage.Clean(rel)
	if err != nil {
		return "", ErrInvalidPath
	}
	return filepath.Join(s.dataPath(id), filepath.FromSlash(clean)), nil
}

func (s *Store) openDataRoot(id string) (*os.Root, error) {
	if !storage.ValidID(id) {
		return nil, ErrInvalidPath
	}
	if _, err := s.Get(context.Background(), id); err != nil {
		return nil, err
	}
	root, err := storage.OpenServer(s.dataDir, id)
	if errors.Is(err, storage.ErrUnsafe) {
		return nil, ErrInvalidPath
	}
	return root, err
}
func safeRel(rel string) (string, error) {
	clean, err := storage.Clean(rel)
	if err != nil {
		return "", ErrInvalidPath
	}
	return clean, nil
}
func rootedEntry(root *os.Root, rel string) (FileEntry, error) {
	info, err := root.Stat(rel)
	if err != nil {
		return FileEntry{}, err
	}
	return FileEntry{Name: path.Base(rel), IsDir: info.IsDir(), SizeBytes: info.Size(), ModifiedAt: info.ModTime()}, nil
}
func (s *Store) ListFiles(id, rel string) (FileListResult, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return FileListResult{}, err
	}
	defer root.Close()
	clean, err := safeRel(rel)
	if err != nil {
		return FileListResult{}, err
	}
	if err = storage.Check(root, clean); err != nil {
		return FileListResult{}, err
	}
	entries, err := fs.ReadDir(root.FS(), clean)
	out := FileListResult{Path: rel, Entries: []FileEntry{}}
	if os.IsNotExist(err) {
		return out, nil
	}
	if err != nil {
		return out, err
	}
	for _, e := range entries {
		info, err := e.Info()
		if err != nil {
			return out, err
		}
		if !info.IsDir() && !info.Mode().IsRegular() {
			continue
		}
		out.Entries = append(out.Entries, FileEntry{Name: e.Name(), IsDir: e.IsDir(), SizeBytes: info.Size(), ModifiedAt: info.ModTime()})
	}
	sort.SliceStable(out.Entries, func(i, j int) bool {
		a, b := out.Entries[i], out.Entries[j]
		if a.IsDir != b.IsDir {
			return a.IsDir
		}
		return a.Name < b.Name
	})
	return out, nil
}
func (s *Store) UploadFile(id, dir, name string, r io.Reader) (FileEntry, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	if name == "" || name == "." || name == ".." || path.Base(name) != name || strings.Contains(name, "\\") {
		return FileEntry{}, ErrInvalidPath
	}
	root, err := s.openDataRoot(id)
	if err != nil {
		return FileEntry{}, err
	}
	defer root.Close()
	rel, err := safeRel(path.Join(dir, name))
	if err != nil {
		return FileEntry{}, err
	}
	_, err = storage.Write(context.Background(), root, rel, r, storage.MaxUploadBytes, 0644)
	if err != nil {
		return FileEntry{}, err
	}
	return rootedEntry(root, rel)
}
func (s *Store) Archive(id, source, name string) (FileEntry, error) {
	return s.ArchiveMultiple(id, []string{source}, "", name)
}
func (s *Store) ArchiveMultiple(id string, sources []string, destDir, name string) (entry FileEntry, err error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return entry, err
	}
	defer root.Close()
	if len(sources) == 0 {
		return entry, ErrInvalidPath
	}
	if name == "" {
		name = "archive"
		if len(sources) == 1 {
			name = path.Base(sources[0])
		}
	}
	if !strings.HasSuffix(strings.ToLower(name), ".zip") {
		name += ".zip"
	}
	if path.Base(name) != name || strings.Contains(name, "\\") {
		return entry, ErrInvalidPath
	}
	target, err := safeRel(path.Join(destDir, name))
	if err != nil {
		return entry, err
	}
	for i, source := range sources {
		source, err = safeRel(source)
		if err != nil {
			return entry, err
		}
		sources[i] = source
		if target == source || source == "." || strings.HasPrefix(target, source+"/") {
			return entry, ErrInvalidPath
		}
		if err = storage.Check(root, source); err != nil {
			return entry, err
		}
		if _, err = root.Stat(source); err != nil {
			return entry, err
		}
	}
	// Temporary output is outside every selected source and published only after ZIP close.
	temp, err := os.CreateTemp("", "mcm-archive-*.zip")
	if err != nil {
		return entry, err
	}
	defer os.Remove(temp.Name())
	defer temp.Close()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	s.RegisterTaskCancel(id, cancel)
	defer s.UnregisterTaskCancel(id)
	zw := zip.NewWriter(temp)
	var count int64
	s.SetTaskProgress(id, TaskProgress{Operation: "archive", ID: name, Stage: "compressing", Message: "Compressing files"})
	defer func() {
		if err != nil {
			s.SetTaskProgress(id, TaskProgress{Operation: "archive", ID: name, Stage: "failed", Error: err.Error(), Message: err.Error()})
		}
	}()
	for _, source := range sources {
		err = fs.WalkDir(root.FS(), source, func(rel string, d fs.DirEntry, walkErr error) error {
			if walkErr != nil {
				return walkErr
			}
			if err := ctx.Err(); err != nil {
				return err
			}
			if err := storage.Check(root, rel); err != nil {
				return err
			}
			info, err := d.Info()
			if err != nil {
				return err
			}
			if !info.IsDir() && !info.Mode().IsRegular() {
				return ErrInvalidPath
			}
			base := path.Base(source)
			if rel != source {
				base = path.Join(base, strings.TrimPrefix(rel, source+"/"))
			}
			hdr, err := zip.FileInfoHeader(info)
			if err != nil {
				return err
			}
			hdr.Name = base
			hdr.Method = zip.Deflate
			if info.IsDir() {
				hdr.Name += "/"
			}
			w, err := zw.CreateHeader(hdr)
			if err != nil {
				return err
			}
			if info.IsDir() {
				return nil
			}
			f, err := storage.OpenRegular(root, rel)
			if err != nil {
				return err
			}
			_, copyErr := io.Copy(w, f)
			closeErr := f.Close()
			if copyErr != nil {
				return copyErr
			}
			if closeErr != nil {
				return closeErr
			}
			count++
			s.SetTaskProgress(id, TaskProgress{Operation: "archive", ID: name, Stage: "compressing", Current: count, Message: "Compressing " + rel})
			return nil
		})
		if err != nil {
			zw.Close()
			return entry, err
		}
	}
	if err = zw.Close(); err != nil {
		return entry, err
	}
	if _, err = temp.Seek(0, 0); err != nil {
		return entry, err
	}
	if _, err = storage.Write(context.Background(), root, target, temp, storage.MaxArchiveBytes, 0644); err != nil {
		return entry, err
	}
	s.SetTaskProgress(id, TaskProgress{Operation: "archive", ID: name, Stage: "completed", Percent: 100, Current: count, Message: "Compression complete"})
	return rootedEntry(root, target)
}
func (s *Store) Unzip(id, archive, dest string) (int, error) {
	return s.UnzipContext(context.Background(), id, archive, dest)
}
func (s *Store) UnzipContext(ctx context.Context, id, archive, dest string) (int, error) {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	s.RegisterTaskCancel(id, cancel)
	defer s.UnregisterTaskCancel(id)
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return 0, err
	}
	defer root.Close()
	archive, err = safeRel(archive)
	if err != nil {
		return 0, err
	}
	f, err := storage.OpenRegular(root, archive)
	if err != nil {
		return 0, err
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil {
		return 0, err
	}
	zr, err := zip.NewReader(f, info.Size())
	if err != nil {
		return 0, ErrInvalidArchive
	}
	if dest == "" {
		dest = path.Dir(archive)
	}
	dest, err = safeRel(dest)
	if err != nil {
		return 0, err
	}
	if len(zr.File) > storage.MaxArchiveEntries {
		return 0, ErrInvalidArchive
	}
	var total uint64
	// Validate every entry before changing the destination.
	for _, member := range zr.File {
		if err := ctx.Err(); err != nil {
			return 0, err
		}
		clean, e := safeRel(member.Name)
		if e != nil || clean == "." || (!member.Mode().IsRegular() && !member.FileInfo().IsDir()) {
			return 0, ErrInvalidArchive
		}
		total += member.UncompressedSize64
		if member.UncompressedSize64 > uint64(storage.MaxArchiveBytes) || total > uint64(storage.MaxArchiveBytes) {
			return 0, ErrInvalidArchive
		}
		if e = storage.Check(root, path.Join(dest, clean)); e != nil {
			return 0, e
		}
	}
	count := 0
	var expanded int64
	for _, member := range zr.File {
		if err := ctx.Err(); err != nil {
			return 0, err
		}
		target := path.Join(dest, member.Name)
		if member.FileInfo().IsDir() {
			err = root.MkdirAll(target, 0755)
		} else {
			var rc io.ReadCloser
			rc, err = member.Open()
			if err != nil {
				return count, ErrInvalidArchive
			}
			var n int64
			n, err = storage.Write(ctx, root, target, rc, storage.MaxArchiveBytes-expanded, 0644)
			rc.Close()
			expanded += n
		}
		if err != nil {
			return count, err
		}
		count++
	}
	return count, nil
}
func (s *Store) DownloadFromURL(id, urlStr, dir, name string) (FileEntry, error) {
	unlock := s.lockWorld(id, false)
	defer unlock()
	u, err := url.Parse(urlStr)
	if err != nil {
		return FileEntry{}, ErrDownloadFailed
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return FileEntry{}, ErrInvalidPath
	}
	if name == "" {
		name = path.Base(u.Path)
	}
	root, err := s.openDataRoot(id)
	if err != nil {
		return FileEntry{}, err
	}
	root.Close()
	client := &http.Client{Timeout: 60 * time.Second}
	resp, err := client.Get(urlStr)
	if err != nil {
		return FileEntry{}, ErrDownloadFailed
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return FileEntry{}, ErrDownloadFailed
	}
	if name == "" || name == "." || name == ".." || path.Base(name) != name {
		return FileEntry{}, ErrInvalidPath
	}
	root, err = s.openDataRoot(id)
	if err != nil {
		return FileEntry{}, err
	}
	defer root.Close()

	rel, err := safeRel(path.Join(dir, name))
	if err != nil {
		return FileEntry{}, err
	}
	if _, err = storage.Write(context.Background(), root, rel, resp.Body, maxDownloadBytes, 0644); err != nil {
		return FileEntry{}, err
	}
	return rootedEntry(root, rel)
}
func (s *Store) DeleteFile(id, rel string) error {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return err
	}
	defer root.Close()
	rel, err = safeRel(rel)
	if err != nil || rel == "." {
		return ErrInvalidPath
	}
	if err = storage.Check(root, rel); err != nil {
		return err
	}
	if _, err = root.Lstat(rel); err != nil {
		return err
	}
	return root.RemoveAll(rel)
}
func (s *Store) Mkdir(id, rel string) (FileEntry, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return FileEntry{}, err
	}
	defer root.Close()
	rel, err = safeRel(rel)
	if err != nil {
		return FileEntry{}, err
	}
	if err = storage.Check(root, rel); err != nil {
		return FileEntry{}, err
	}
	if err = root.MkdirAll(rel, 0755); err != nil {
		return FileEntry{}, err
	}
	return rootedEntry(root, rel)
}

func (s *Store) copySingleFile(root *os.Root, src, dst string, perm fs.FileMode) error {
	if err := root.MkdirAll(path.Dir(dst), 0755); err != nil {
		return err
	}
	srcFile, err := storage.OpenRegular(root, src)
	if err != nil {
		return err
	}
	defer srcFile.Close()

	dstFile, err := root.OpenFile(dst, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, perm)
	if err != nil {
		return err
	}
	defer dstFile.Close()

	if _, err := io.Copy(dstFile, srcFile); err != nil {
		return err
	}
	return dstFile.Sync()
}

func (s *Store) copyDir(root *os.Root, src, dst string) error {
	if err := root.MkdirAll(dst, 0755); err != nil {
		return err
	}
	return fs.WalkDir(root.FS(), src, func(p string, d fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		relSub := strings.TrimPrefix(p, src)
		relSub = strings.TrimPrefix(relSub, "/")
		if relSub == "" {
			return nil
		}
		target := path.Join(dst, relSub)
		if d.IsDir() {
			return root.MkdirAll(target, 0755)
		}
		info, err := d.Info()
		if err != nil {
			return err
		}
		return s.copySingleFile(root, p, target, info.Mode().Perm())
	})
}

func resolveCopyDestination(root *os.Root, target string) string {
	if _, err := root.Stat(target); err != nil {
		return target
	}
	dir := path.Dir(target)
	base := path.Base(target)
	ext := path.Ext(base)
	nameWithoutExt := strings.TrimSuffix(base, ext)

	candidateBase := fmt.Sprintf("%s (copy)%s", nameWithoutExt, ext)
	candidate := path.Join(dir, candidateBase)
	if _, err := root.Stat(candidate); err != nil {
		return candidate
	}

	for i := 2; i < 1000; i++ {
		candidateBase := fmt.Sprintf("%s (copy %d)%s", nameWithoutExt, i, ext)
		candidate = path.Join(dir, candidateBase)
		if _, err := root.Stat(candidate); err != nil {
			return candidate
		}
	}
	return candidate
}

// CopyFile duplicates a file or directory at destination. If destination is an existing
// directory, the source is copied into that directory with its base name.
func (s *Store) CopyFile(id, source, destination string) (FileEntry, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return FileEntry{}, err
	}
	defer root.Close()

	srcClean, err := safeRel(source)
	if err != nil || srcClean == "." {
		return FileEntry{}, ErrInvalidPath
	}
	dstClean, err := safeRel(destination)
	if err != nil || dstClean == "." {
		return FileEntry{}, ErrInvalidPath
	}

	if strings.HasSuffix(destination, "/") {
		if err := root.MkdirAll(dstClean, 0755); err != nil {
			return FileEntry{}, err
		}
	}
	if dstInfo, err := root.Stat(dstClean); err == nil && dstInfo.IsDir() {
		dstClean = path.Join(dstClean, path.Base(srcClean))
	}
	if srcClean == dstClean {
		dstClean = resolveCopyDestination(root, dstClean)
	}

	if strings.HasPrefix(dstClean, srcClean+"/") {
		return FileEntry{}, fmt.Errorf("%w: cannot copy directory into itself", ErrInvalidPath)
	}
	if err = storage.Check(root, srcClean); err != nil {
		return FileEntry{}, err
	}
	if err = storage.Check(root, dstClean); err != nil {
		return FileEntry{}, err
	}

	srcInfo, err := root.Stat(srcClean)
	if err != nil {
		return FileEntry{}, err
	}

	if srcInfo.IsDir() {
		if err := s.copyDir(root, srcClean, dstClean); err != nil {
			return FileEntry{}, err
		}
	} else {
		if err := s.copySingleFile(root, srcClean, dstClean, srcInfo.Mode().Perm()); err != nil {
			return FileEntry{}, err
		}
	}

	return rootedEntry(root, dstClean)
}

// MoveFile renames or moves a file or directory to destination. If destination is an
// existing directory, the source is moved into that directory with its base name.
func (s *Store) MoveFile(id, source, destination string) (FileEntry, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return FileEntry{}, err
	}
	defer root.Close()

	srcClean, err := safeRel(source)
	if err != nil || srcClean == "." {
		return FileEntry{}, ErrInvalidPath
	}
	dstClean, err := safeRel(destination)
	if err != nil || dstClean == "." {
		return FileEntry{}, ErrInvalidPath
	}
	if srcClean == dstClean {
		return rootedEntry(root, dstClean)
	}
	if strings.HasPrefix(dstClean, srcClean+"/") {
		return FileEntry{}, fmt.Errorf("%w: cannot move directory into itself", ErrInvalidPath)
	}
	if err = storage.Check(root, srcClean); err != nil {
		return FileEntry{}, err
	}
	if err = storage.Check(root, dstClean); err != nil {
		return FileEntry{}, err
	}

	if strings.HasSuffix(destination, "/") {
		if err := root.MkdirAll(dstClean, 0755); err != nil {
			return FileEntry{}, err
		}
	}
	if dstInfo, err := root.Stat(dstClean); err == nil && dstInfo.IsDir() {
		dstClean = path.Join(dstClean, path.Base(srcClean))
		if srcClean == dstClean {
			return rootedEntry(root, dstClean)
		}
		if err = storage.Check(root, dstClean); err != nil {
			return FileEntry{}, err
		}
	}

	if err := root.MkdirAll(path.Dir(dstClean), 0755); err != nil {
		return FileEntry{}, err
	}

	if err := root.Rename(srcClean, dstClean); err != nil {
		return FileEntry{}, err
	}
	return rootedEntry(root, dstClean)
}

// CopyMultiple copies multiple sources into destinationDir.
func (s *Store) CopyMultiple(id string, sources []string, destinationDir string) ([]FileEntry, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return nil, err
	}
	defer root.Close()

	dstDir, err := safeRel(destinationDir)
	if err != nil {
		return nil, ErrInvalidPath
	}
	if err := root.MkdirAll(dstDir, 0755); err != nil {
		return nil, err
	}

	var results []FileEntry
	for _, source := range sources {
		srcClean, err := safeRel(source)
		if err != nil || srcClean == "." {
			return nil, ErrInvalidPath
		}
		target := path.Join(dstDir, path.Base(srcClean))
		if strings.HasPrefix(target, srcClean+"/") {
			return nil, fmt.Errorf("%w: cannot copy directory into itself", ErrInvalidPath)
		}
		target = resolveCopyDestination(root, target)
		if err = storage.Check(root, srcClean); err != nil {
			return nil, err
		}
		if err = storage.Check(root, target); err != nil {
			return nil, err
		}
		srcInfo, err := root.Stat(srcClean)
		if err != nil {
			return nil, err
		}
		if srcInfo.IsDir() {
			if err := s.copyDir(root, srcClean, target); err != nil {
				return nil, err
			}
		} else {
			if err := s.copySingleFile(root, srcClean, target, srcInfo.Mode().Perm()); err != nil {
				return nil, err
			}
		}
		entry, err := rootedEntry(root, target)
		if err == nil {
			results = append(results, entry)
		}
	}
	return results, nil
}

// MoveMultiple moves multiple sources into destinationDir.
func (s *Store) MoveMultiple(id string, sources []string, destinationDir string) ([]FileEntry, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return nil, err
	}
	defer root.Close()

	dstDir, err := safeRel(destinationDir)
	if err != nil {
		return nil, ErrInvalidPath
	}
	if err := root.MkdirAll(dstDir, 0755); err != nil {
		return nil, err
	}

	var results []FileEntry
	for _, source := range sources {
		srcClean, err := safeRel(source)
		if err != nil || srcClean == "." {
			return nil, ErrInvalidPath
		}
		target := path.Join(dstDir, path.Base(srcClean))
		if srcClean == target {
			continue
		}
		if strings.HasPrefix(target, srcClean+"/") {
			return nil, fmt.Errorf("%w: cannot move directory into itself", ErrInvalidPath)
		}
		if err = storage.Check(root, srcClean); err != nil {
			return nil, err
		}
		if err = storage.Check(root, target); err != nil {
			return nil, err
		}
		if err := root.Rename(srcClean, target); err != nil {
			return nil, err
		}
		entry, err := rootedEntry(root, target)
		if err == nil {
			results = append(results, entry)
		}
	}
	return results, nil
}
func (s *Store) Rename(id, rel, name string) (FileEntry, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return FileEntry{}, err
	}
	defer root.Close()
	rel, err = safeRel(rel)
	if err != nil || rel == "." {
		return FileEntry{}, ErrInvalidPath
	}
	if name == "" || name == "." || name == ".." || path.Base(name) != name || strings.Contains(name, "\\") {
		return FileEntry{}, ErrInvalidPath
	}
	target := path.Join(path.Dir(rel), name)
	if err = storage.Check(root, rel); err != nil {
		return FileEntry{}, err
	}
	if err = storage.Check(root, target); err != nil {
		return FileEntry{}, err
	}
	if err = root.Rename(rel, target); err != nil {
		return FileEntry{}, err
	}
	return rootedEntry(root, target)
}
func (s *Store) OpenFileForDownload(id, rel string) (*os.File, string, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	root, err := s.openDataRoot(id)
	if err != nil {
		return nil, "", err
	}
	defer root.Close()
	rel, err = safeRel(rel)
	if err != nil {
		return nil, "", err
	}
	info, err := root.Stat(rel)
	if err != nil {
		return nil, "", err
	}
	if info.IsDir() {
		return nil, "", ErrIsDirectory
	}
	f, err := storage.OpenRegular(root, rel)
	return f, path.Base(rel), err
}

const maxEditBytes int64 = 5 << 20
const MaxEditRequestBytes int64 = 6*maxEditBytes + 4096 // JSON can escape each content byte as \uXXXX.
func (s *Store) ReadFileContent(id, rel string) (string, error) {
	f, _, err := s.OpenFileForDownload(id, rel)
	if err != nil {
		return "", err
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, maxEditBytes+1))
	if int64(len(data)) > maxEditBytes {
		return "", ErrReadTooLarge
	}
	return string(data), err
}
func (s *Store) WriteFileContent(id, rel, content string, expectedRevision ...string) (FileEntry, error) {
	unlockWorld := s.lockWorld(id, false)
	defer unlockWorld()

	s.fileEditMu.Lock()
	defer s.fileEditMu.Unlock()
	if int64(len(content)) > maxEditBytes {
		return FileEntry{}, ErrReadTooLarge
	}
	rel, err := safeRel(rel)
	if err != nil || rel == "." {
		return FileEntry{}, ErrInvalidPath
	}
	root, err := s.openDataRoot(id)
	if err != nil {
		return FileEntry{}, err
	}
	defer root.Close()
	if len(expectedRevision) > 0 {
		current, err := s.readFileSnapshotLocked(root, rel)
		if errors.Is(err, os.ErrNotExist) || errors.Is(err, ErrPathNotFound) {
			current = FileContent{Path: rel}
		} else if err != nil {
			return FileEntry{}, err
		}
		if current.Revision != expectedRevision[0] {
			return FileEntry{}, &FileConflictError{Current: current}
		}
	}
	if _, err = storage.Write(context.Background(), root, rel, strings.NewReader(content), maxEditBytes, 0644); err != nil {
		return FileEntry{}, err
	}
	entry, err := rootedEntry(root, rel)
	entry.Revision = fileRevision(content)
	return entry, err
}

func (s *Store) readFileSnapshotLocked(root *os.Root, rel string) (FileContent, error) {
	f, err := storage.OpenRegular(root, rel)
	if err != nil {
		return FileContent{}, err
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, maxEditBytes+1))
	if err != nil {
		return FileContent{}, err
	}
	if int64(len(data)) > maxEditBytes {
		return FileContent{}, ErrReadTooLarge
	}
	content := string(data)
	return FileContent{Path: rel, Content: content, Revision: fileRevision(content), Exists: true}, nil
}
