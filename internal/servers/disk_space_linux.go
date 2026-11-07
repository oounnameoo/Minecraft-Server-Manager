//go:build linux

package servers

import (
	"errors"
	"syscall"
)

func dataDiskSpace(path string) (DataDisk, error) {
	var stat syscall.Statfs_t
	if err := syscall.Statfs(path, &stat); err != nil {
		return DataDisk{}, err
	}
	if stat.Blocks == 0 || stat.Bsize <= 0 {
		return DataDisk{}, errors.New("filesystem size is not reported")
	}
	blockSize := uint64(stat.Bsize)
	if stat.Frsize > 0 {
		blockSize = uint64(stat.Frsize)
	}
	blocks := uint64(stat.Blocks)
	free := min(uint64(stat.Bfree), blocks)
	available := min(uint64(stat.Bavail), free)
	return DataDisk{
		Path: path, TotalBytes: blocks * blockSize,
		UsedBytes: (blocks - free) * blockSize, AvailableBytes: available * blockSize,
	}, nil
}
