## Git 多人协作学习笔记

写在前面：本文仅使用命令行指令实现操作。由于容器中的环境无法正常使用 git，因此建议大家也借此机会使用命令行指令来进行 git 管理，这也是一个锻炼你终端使用的好机会

希望大家能自己手敲命令行指令而非复制，这些指令我们会经常用到

希望大家能自己手敲命令行指令而非复制，这些指令我们会经常用到

希望大家能自己手敲命令行指令而非复制，这些指令我们会经常用到

> 相关文档
>
> https://www.runoob.com/git/git-tutorial.html
> https://git-scm.com/book/zh/v2

------

### 一、Git 多人协作的核心目标

多人同时开发同一个项目时，Git 用来解决：

- 代码互相覆盖
- 主分支不稳定
- 修改来源不清楚
- 回滚困难
- 多人功能并行开发困难

核心原则：主分支保持稳定，开发工作在各自分支进行

------

### 二、推荐分支命名

```text
main          稳定可运行版本
dev           集成测试分支
feat/xxx      新功能开发
fix/xxx       Bug 修复
docs/xxx      文档更新
refactor/xxx  重构代码
```

分支命名示例：

```text
feat/fs         开发文件系统
feat/scheduler  开发线程调度算法
fix/pagefault   修复页错误
docs/readme     添加 README 文档
```

------

### 三、标准多人协作流程（最重要）

```text
1. 拉取最新代码
2. 创建个人分支
3. 在分支开发
4. 提交代码
5. Push 到远程仓库
6. 发 Pull Request(PR) —— 大家到这一步就行了
7. Code Review
8. Merge 到主分支
```

------

### 四、常用命令总结

#### 1. 拉取最新代码

```bash
git pull origin main
```

`pull` 后面跟的是远程仓库的名字，他代表你 clone 时的远程仓库。之后我们会转变到比赛的远程仓库。该指令用于同步远程仓库上 main 分支最新内容

------

#### 2. 创建并切换分支

```bash
git checkout -b xxx
git switch -c xxx
```

二者皆可，从当前分支创建一个新分支，独立开发新功能

------

#### 3. 查看当前分支

```bash
git branch
```

------

#### 4. 提交代码

```bash
git add . # 跟踪和追踪所有文件
git commit -m "对于此次提交的描述"
```


对于 commit 填写的消息部分可以参考

------

#### 5. 推送到远程仓库

首次推送，`-u` 表示默认设置

```bash
git push -u origin feat/filesystem
```

之后可以简化指令来推送代码。需要注意如果你的分支发生了更改就**不能无脑 push**

```bash
git push
```

------

### 五、Pull Request(PR)

PR 表示：我已经开发完成，请求把我的分支合并进 main

#### PR 内容建议

title:

```text
实现文件系统目录读取功能
```

descirption：

```text
1. 增加 inode lookup
2. 支持 ls
3. 修复路径解析 bug
```

------

### 六、小结

到此为止基本上就是大家开发一个功能的流程，好好学习实践一下。

你 PR 之后我会尝试将你的分支与我们项目的 main 分支合并，在这个过程中可能会出现问题

------

### 七、冲突（Conflict）

为什么会冲突？

多人同时修改同一位置代码。

如何解决？

首先我们在开发过程中需要多交流，由于我也是第一次接触团队协作，加上内核中有极多的耦合的代码，我们在功能开发时必须多多交流，从而减少从图
即使文件冲突存在，我们可以比较文件差异，选择保留其中的一部分代码

------

### 八、补充

必须掌握的 git 指令：

```bash
git clone
git pull
git checkout -b 分支名
git add .
git commit -m ""
git push
git branch
```

通用每天的开发流程（参考）：

```bash
git checkout main
git pull

git checkout -b feat/xxx

(开发)

git add .
git commit -m "feat: xxx"

git push origin feat/xxx
```
